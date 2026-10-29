//! What the crate's tests share: the orders API's spec as the harness's
//! FastAPI app serves it (v1.4.2) and its next version, a Swashbuckle page,
//! and a throwaway Contoso test CA and leaf.

use serde_json::{Value, json};

pub(crate) const SWASHBUCKLE_PAGE: &str = "<!DOCTYPE html><html><body><div id=\"swagger-ui\"></div><script src=\"index.js\" charset=\"utf-8\"></script></body></html>";

pub(crate) fn leaf_der() -> Vec<u8> {
    include_bytes!("testdata/leaf.der").to_vec()
}

pub(crate) fn ca_der() -> Vec<u8> {
    include_bytes!("testdata/ca.der").to_vec()
}

fn order(note: bool) -> Value {
    let mut properties = json!({
        "id": {"type": "integer"},
        "status": {"type": "string", "enum": ["open", "shipped", "on_hold"]},
        "customer_id": {"type": "integer"},
        "lines": {"type": "array", "items": {"$ref": "#/components/schemas/Line"}}
    });
    if note {
        properties["note"] = json!({"anyOf": [{"type": "string"}, {"type": "null"}]});
    }
    json!({"type": "object", "required": ["id", "status", "customer_id", "lines"], "properties": properties})
}

pub(crate) fn spec_v1() -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {"title": "Contoso Orders API", "version": "1.4.2"},
        "security": [{"Bearer": []}],
        "paths": {
            "/orders": {
                "get": {
                    "summary": "List orders", "operationId": "list_orders", "tags": ["orders"], "security": [],
                    "parameters": [{"in": "query", "name": "status", "schema": {"type": "string", "enum": ["open", "shipped", "on_hold"]}}],
                    "responses": {"200": {"description": "Orders", "content": {"application/json": {"schema": {"type": "array", "items": {"$ref": "#/components/schemas/Order"}}}}}}
                },
                "post": {
                    "summary": "Create an order", "operationId": "create_order", "tags": ["orders"],
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/OrderIn"}}}},
                    "responses": {"201": {"description": "Created", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Order"}}}}, "422": {"description": "Validation Error"}}
                }
            },
            "/orders/{order_id}": {
                "get": {
                    "summary": "Get an order", "operationId": "get_order", "tags": ["orders"],
                    "parameters": [{"in": "path", "name": "order_id", "required": true, "schema": {"type": "integer"}}],
                    "responses": {"200": {"description": "The order", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Order"}}}}, "404": {"description": "Not found"}}
                },
                "delete": {
                    "summary": "Delete an order", "operationId": "delete_order", "tags": ["orders"],
                    "parameters": [{"in": "path", "name": "order_id", "required": true, "schema": {"type": "integer"}}],
                    "responses": {"204": {"description": "Deleted"}}
                }
            }
        },
        "components": {
            "schemas": {
                "Order": order(true),
                "OrderIn": {"type": "object", "required": ["customer_id", "lines"], "properties": {
                    "customer_id": {"type": "integer"},
                    "lines": {"type": "array", "items": {"$ref": "#/components/schemas/Line"}},
                    "note": {"anyOf": [{"type": "string", "maxLength": 200}, {"type": "null"}]}
                }},
                "Line": {"type": "object", "required": ["sku"], "properties": {"sku": {"type": "string"}, "qty": {"type": "integer", "default": 1}}}
            },
            "securitySchemes": {"Bearer": {"type": "http", "scheme": "bearer", "bearerFormat": "JWT"}}
        }
    })
}

/// v2.0.0: `currency` required in a new order, `on_hold` gone from the
/// status filter, DELETE gone, `note` gone from an order; a fulfilment
/// operation and an optional `customer_id` filter added.
pub(crate) fn spec_v2() -> Value {
    let mut spec = spec_v1();
    spec["info"]["version"] = json!("2.0.0");
    spec["components"]["schemas"]["Order"] = order(false);
    let order_in = &mut spec["components"]["schemas"]["OrderIn"];
    order_in["properties"]["currency"] = json!({"type": "string", "enum": ["EUR", "USD"]});
    order_in["required"] = json!(["customer_id", "lines", "currency"]);
    let list = &mut spec["paths"]["/orders"]["get"];
    list["parameters"] = json!([
        {"in": "query", "name": "status", "schema": {"type": "string", "enum": ["open", "shipped"]}},
        {"in": "query", "name": "customer_id", "schema": {"type": "integer"}}
    ]);
    spec["paths"]["/orders/{order_id}"]
        .as_object_mut()
        .unwrap()
        .remove("delete");
    spec["paths"]["/orders/{order_id}/fulfilment"] = json!({"get": {
        "summary": "Where an order is", "operationId": "get_fulfilment", "tags": ["orders"],
        "parameters": [{"in": "path", "name": "order_id", "required": true, "schema": {"type": "integer"}}],
        "responses": {"200": {"description": "Fulfilment"}}
    }});
    spec
}
