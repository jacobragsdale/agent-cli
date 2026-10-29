"""The orders API on Django REST Framework with drf-spectacular.

One file is settings, URLs and views. The URL patterns are the ones in
drf-spectacular's README; the router is DRF's DefaultRouter, whose trailing
slashes and API root at / are what a DRF project ships.
"""

import sys

from django.conf import settings

settings.configure(
    DEBUG=False,
    ALLOWED_HOSTS=["*"],
    ROOT_URLCONF=__name__,
    SECRET_KEY="agent-cli-openapi-throwaway",
    INSTALLED_APPS=[
        "django.contrib.contenttypes",
        "django.contrib.auth",
        "django.contrib.staticfiles",
        "rest_framework",
        "drf_spectacular",
    ],
    MIDDLEWARE=["django.middleware.common.CommonMiddleware"],
    TEMPLATES=[{"BACKEND": "django.template.backends.django.DjangoTemplates", "APP_DIRS": True}],
    STATIC_URL="static/",
    DATABASES={},
    REST_FRAMEWORK={
        "DEFAULT_SCHEMA_CLASS": "drf_spectacular.openapi.AutoSchema",
        "DEFAULT_AUTHENTICATION_CLASSES": [],
        "DEFAULT_PERMISSION_CLASSES": [],
        "UNAUTHENTICATED_USER": None,
    },
    SPECTACULAR_SETTINGS={"TITLE": "Contoso Orders API", "VERSION": "1.0.0"},
)

import django  # noqa: E402

django.setup()

from django.urls import include, path  # noqa: E402
from drf_spectacular.utils import OpenApiParameter, extend_schema  # noqa: E402
from drf_spectacular.views import (  # noqa: E402
    SpectacularAPIView,
    SpectacularRedocView,
    SpectacularSwaggerView,
)
from rest_framework import routers, serializers, status, viewsets  # noqa: E402
from rest_framework.response import Response  # noqa: E402

STATUSES = ["open", "shipped", "cancelled", "on_hold"]


class LineSerializer(serializers.Serializer):
    sku = serializers.CharField()
    qty = serializers.IntegerField()


class OrderInSerializer(serializers.Serializer):
    customer_id = serializers.IntegerField()
    lines = LineSerializer(many=True)
    note = serializers.CharField(required=False, allow_null=True)


class OrderSerializer(OrderInSerializer):
    id = serializers.IntegerField()
    status = serializers.ChoiceField(choices=STATUSES)


ORDERS = [
    {"id": i, "customer_id": 100 + i, "status": "open", "lines": [{"sku": f"SKU-00{i}", "qty": i}], "note": None}
    for i in (1, 2, 3)
]


class OrderViewSet(viewsets.ViewSet):
    lookup_value_regex = r"\d+"

    @extend_schema(
        parameters=[OpenApiParameter("status", str, enum=STATUSES)],
        responses=OrderSerializer(many=True),
    )
    def list(self, request):
        wanted = request.query_params.get("status")
        return Response([o for o in ORDERS if wanted in (None, o["status"])])

    @extend_schema(parameters=[OpenApiParameter("id", int, OpenApiParameter.PATH)], responses=OrderSerializer)
    def retrieve(self, request, pk=None):
        for order in ORDERS:
            if str(order["id"]) == pk:
                return Response(order)
        return Response({"detail": f"order {pk} not found"}, status.HTTP_404_NOT_FOUND)

    @extend_schema(request=OrderInSerializer, responses={201: OrderSerializer})
    def create(self, request):
        body = OrderInSerializer(data=request.data)
        body.is_valid(raise_exception=True)
        order = {"id": len(ORDERS) + 1, "status": "open", "note": None, **body.validated_data}
        ORDERS.append(order)
        return Response(order, status.HTTP_201_CREATED)


router = routers.DefaultRouter()
router.register("orders", OrderViewSet, basename="order")

urlpatterns = [
    path("", include(router.urls)),
    path("api/schema/", SpectacularAPIView.as_view(), name="schema"),
    path("api/schema/swagger-ui/", SpectacularSwaggerView.as_view(url_name="schema"), name="swagger-ui"),
    path("api/schema/redoc/", SpectacularRedocView.as_view(url_name="schema"), name="redoc"),
]

if __name__ == "__main__":
    from django.core.management import execute_from_command_line

    execute_from_command_line([sys.argv[0], *sys.argv[1:]])
