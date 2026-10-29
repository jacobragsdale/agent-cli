"""The rich orders API the http domain's live suite runs against.

One file serves three containers: fastapi (1.4.2), fastapi-v2 (2.0.0, set by
ORDERS_API_VERSION) and echo (a second origin for cross-origin redirects).
v2's spec changes are listed in scripts/openapi-up.sh; every harness route
(fail, slow, redirect, echo, binary, text, admin) is the same in both and is
left out of the spec, so the v1/v2 diff holds only the intended changes.
"""

import asyncio
import base64
import os
import time
from enum import Enum
from typing import Annotated, Literal

from fastapi import FastAPI, Header, HTTPException, Query, Request, Response
from fastapi.responses import JSONResponse, PlainTextResponse, RedirectResponse
from pydantic import BaseModel, ConfigDict

VERSION = os.environ.get("ORDERS_API_VERSION", "1.4.2")
V2 = VERSION.startswith("2.")
# Where /redirect/external sends a client: the echo container as the host sees it.
ECHO_URL = os.environ.get("ECHO_URL", "http://127.0.0.1:58089")
TOKEN = "contoso-test-token"
API_KEY = "contoso-api-key"
BASIC = "Basic " + base64.b64encode(b"agent:basic-pass").decode()
# A W3C traceparent whose trace id's low 64 bits are 1846030217602524919, the
# decimal form Datadog prints.
TRACEPARENT = "00-0000000000000000199e6ab3c4d5e6f7-00f067aa0ba902b7-01"

MODEL = ConfigDict(
    extra="forbid",
    frozen=True,
    validate_default=True,
    revalidate_instances="always",
    allow_inf_nan=False,
)


class OrderStatus(str, Enum):
    open = "open"
    shipped = "shipped"
    cancelled = "cancelled"
    on_hold = "on_hold"


# The status filter is its own inline enum so v2 can narrow it without
# touching the response schema.
if V2:
    StatusFilter = Literal["open", "shipped", "cancelled"]
else:
    StatusFilter = Literal["open", "shipped", "cancelled", "on_hold"]


class Currency(str, Enum):
    EUR = "EUR"
    USD = "USD"


class Line(BaseModel):
    model_config = MODEL
    sku: str
    qty: int


if V2:

    class OrderIn(BaseModel):
        model_config = MODEL
        customer_id: int
        lines: list[Line]
        currency: Currency
        note: str | None = None

    class Order(BaseModel):
        model_config = MODEL
        id: int
        customer_id: int
        status: OrderStatus
        lines: list[Line]

else:

    class OrderIn(BaseModel):
        model_config = MODEL
        customer_id: int
        lines: list[Line]
        note: str | None = None

    class Order(BaseModel):
        model_config = MODEL
        id: int
        customer_id: int
        status: OrderStatus
        lines: list[Line]
        note: str | None = None


class OrderPatch(BaseModel):
    model_config = MODEL
    status: OrderStatus | None = None
    note: str | None = None


class Search(BaseModel):
    model_config = MODEL
    status: OrderStatus | None = None
    customer_id: int | None = None
    sku: str | None = None


class Fulfilment(BaseModel):
    model_config = MODEL
    order_id: int
    carrier: str
    tracking: str
    shipped: bool


class Health(BaseModel):
    model_config = MODEL
    status: str
    version: str


class Token(BaseModel):
    model_config = MODEL
    access_token: str
    token_type: str
    expires_in: int


def seed() -> dict[int, dict[str, object]]:
    """600 open orders (for output bounding), then a few of every other status."""
    statuses = (
        [OrderStatus.open] * 600
        + [OrderStatus.shipped] * 20
        + [OrderStatus.cancelled] * 10
        + [OrderStatus.on_hold] * 5
    )
    orders: dict[int, dict[str, object]] = {}
    for i, status in enumerate(statuses, start=1):
        orders[i] = {
            "id": i,
            "customer_id": 100 + i % 7,
            "status": status.value,
            "lines": [{"sku": f"SKU-{i % 13:03}", "qty": i % 5 + 1}],
            "note": None,
        }
    orders[1]["note"] = ("Leave at the loading dock behind building 4. " * 120)[:5120]
    return orders


class State:
    def __init__(self) -> None:
        self.reset()

    def reset(self) -> None:
        self.orders = seed()
        self.throttled: set[str] = set()
        self.version = VERSION
        self.deploy: tuple[str, float] | None = None

    def current_version(self) -> str:
        if self.deploy and time.monotonic() >= self.deploy[1]:
            self.version, self.deploy = self.deploy[0], None
        return self.version


state = State()
app = FastAPI(title="Contoso Orders API", version=VERSION)


def shown(order: dict[str, object]) -> dict[str, object]:
    return {k: v for k, v in order.items() if k in Order.model_fields}


def skus(order: dict[str, object]) -> list[object]:
    lines = order["lines"]
    return [line["sku"] for line in lines] if isinstance(lines, list) else []


def find(order_id: int) -> dict[str, object]:
    if order_id not in state.orders:
        raise HTTPException(404, f"order {order_id} not found")
    return state.orders[order_id]


@app.get("/orders", response_model=list[Order])
def list_orders(
    status: StatusFilter | None = None,
    customer_id: Annotated[int | None, Query(include_in_schema=V2)] = None,
) -> list[dict[str, object]]:
    return [
        shown(o)
        for o in state.orders.values()
        if (status is None or o["status"] == status)
        and (customer_id is None or o["customer_id"] == customer_id)
    ]


@app.get("/orders/{order_id}", response_model=Order)
def get_order(order_id: int) -> dict[str, object]:
    return shown(find(order_id))


@app.post("/orders", response_model=Order, status_code=201)
def create_order(order: OrderIn) -> dict[str, object]:
    new_id = max(state.orders) + 1
    state.orders[new_id] = {
        "id": new_id,
        "status": OrderStatus.open.value,
        **order.model_dump(mode="json", exclude={"currency"}),
    }
    return shown(state.orders[new_id])


@app.post("/orders/search", response_model=list[Order])
def search_orders(query: Search) -> list[dict[str, object]]:
    return [
        shown(o)
        for o in state.orders.values()
        if (query.status is None or o["status"] == query.status.value)
        and (query.customer_id is None or o["customer_id"] == query.customer_id)
        and (query.sku is None or query.sku in skus(o))
    ]


@app.put("/orders/{order_id}", response_model=Order)
def replace_order(order_id: int, order: OrderIn) -> dict[str, object]:
    old = find(order_id)
    state.orders[order_id] = {
        "id": order_id,
        "status": old["status"],
        **order.model_dump(mode="json", exclude={"currency"}),
    }
    return shown(state.orders[order_id])


@app.patch("/orders/{order_id}", response_model=Order)
def update_order(order_id: int, patch: OrderPatch) -> dict[str, object]:
    order = find(order_id)
    order.update(patch.model_dump(mode="json", exclude_unset=True))
    return shown(order)


if V2:

    @app.get("/orders/{order_id}/fulfilment", response_model=Fulfilment)
    def get_fulfilment(order_id: int) -> Fulfilment:
        order = find(order_id)
        return Fulfilment(
            order_id=order_id,
            carrier="Contoso Freight",
            tracking=f"CF{order_id:08}",
            shipped=order["status"] == OrderStatus.shipped.value,
        )

else:

    @app.delete("/orders/{order_id}", status_code=204)
    def delete_order(order_id: int) -> Response:
        find(order_id)
        del state.orders[order_id]
        return Response(status_code=204)


@app.get("/healthz", response_model=Health)
def healthz() -> Health:
    return Health(status="ok", version=state.current_version())


@app.post("/token", response_model=Token)
def token() -> Token:
    return Token(access_token="secret-value", token_type="Bearer", expires_in=3600)


def unauthorized(scheme: str) -> JSONResponse:
    return JSONResponse(
        {"detail": "not authenticated"}, 401, headers={"WWW-Authenticate": scheme}
    )


@app.get("/secure/orders", response_model=list[Order], responses={401: {}})
def secure_orders(authorization: Annotated[str | None, Header()] = None) -> object:
    if authorization != f"Bearer {TOKEN}":
        return unauthorized("Bearer")
    return [shown(o) for o in list(state.orders.values())[:3]]


@app.get("/basic", responses={401: {}})
def basic(authorization: Annotated[str | None, Header()] = None) -> object:
    if authorization != BASIC:
        return unauthorized('Basic realm="contoso"')
    return {"user": "agent"}


@app.get("/apikey", responses={401: {}})
def apikey(x_api_key: Annotated[str | None, Header()] = None) -> object:
    if x_api_key != API_KEY:
        return JSONResponse({"detail": "missing or wrong X-Api-Key"}, 401)
    return {"key": "ok"}


# The harness routes below stay out of the spec.
harness = {"include_in_schema": False}


@app.get("/fail/500", **harness)
def fail_500() -> JSONResponse:
    return JSONResponse({"error": "database unavailable"}, 500)


@app.get("/fail/503", **harness)
def fail_503() -> JSONResponse:
    return JSONResponse(
        {"error": "upstream unavailable"},
        503,
        headers={"traceparent": TRACEPARENT, "x-request-id": "req-contoso-503"},
    )


@app.get("/fail/429", **harness)
def fail_429(key: str = "default") -> JSONResponse:
    if key not in state.throttled:
        state.throttled.add(key)
        return JSONResponse({"error": "slow down"}, 429, headers={"Retry-After": "1"})
    return JSONResponse({"status": "ok", "key": key})


@app.get("/fail/400", **harness)
def fail_400() -> JSONResponse:
    return JSONResponse({"error": "customer_id must be positive"}, 400)


@app.get("/fail/401", **harness)
def fail_401() -> JSONResponse:
    return JSONResponse(
        {"error": "token expired"},
        401,
        headers={"WWW-Authenticate": 'Bearer error="invalid_token"'},
    )


@app.get("/fail/409", **harness)
def fail_409() -> JSONResponse:
    return JSONResponse({"error": "order 1 was changed by someone else"}, 409)


@app.get("/slow", **harness)
async def slow(seconds: float = 5) -> dict[str, float]:
    await asyncio.sleep(seconds)
    return {"slept": seconds}


# Each redirect takes ?status= (302 by default) so 307/308, which keep the
# method and body, can be tried too.
@app.api_route("/redirect", methods=["GET", "POST"], **harness)
def redirect(status: int = 302, to: str = "/orders/1") -> RedirectResponse:
    return RedirectResponse(to, status)


@app.api_route("/redirect/external", methods=["GET", "POST"], **harness)
def redirect_external(status: int = 302) -> RedirectResponse:
    return RedirectResponse(f"{ECHO_URL}/echo", status)


@app.get("/redirect/loop", **harness)
def redirect_loop() -> RedirectResponse:
    return RedirectResponse("/redirect/loop", 302)


@app.api_route("/echo", methods=["GET", "POST", "PUT", "PATCH", "DELETE"], **harness)
async def echo(request: Request) -> dict[str, object]:
    headers: dict[str, str] = {}
    for name, value in request.headers.items():
        headers[name] = f"{headers[name]}, {value}" if name in headers else value
    return {
        "method": request.method,
        "path": request.url.path,
        "headers": headers,
        "query": dict(request.query_params),
        "body": (await request.body()).decode(errors="replace"),
    }


@app.get("/binary", **harness)
def binary() -> Response:
    return Response(bytes(range(256)) * 8, media_type="application/octet-stream")


@app.get("/text", **harness)
def text() -> PlainTextResponse:
    lines = (f"line {i:05}: the quick brown fox jumps over the lazy dog\n" for i in range(1, 10_000))
    body, size = [], 0
    for line in lines:
        if size + len(line) > 40 * 1024:
            break
        body.append(line)
        size += len(line)
    return PlainTextResponse("".join(body))


@app.post("/admin/deploy", **harness)
def deploy(version: str, after: float = 0) -> dict[str, object]:
    state.deploy = (version, time.monotonic() + after)
    return {"deploying": version, "after": after}


@app.post("/admin/reset", **harness)
def reset() -> dict[str, str]:
    state.reset()
    return {"status": "reset"}

