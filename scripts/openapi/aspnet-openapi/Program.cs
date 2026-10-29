// The orders API on ASP.NET Core 9 with the built-in OpenAPI document
// (MapOpenApi) and Scalar's UI, both at their defaults and, unlike the
// webapi template, outside its Development-only guard.
using System.ComponentModel.DataAnnotations;
using System.Text.Json;
using Scalar.AspNetCore;

var builder = WebApplication.CreateBuilder(args);
builder.Services.AddOpenApi();
builder.Services.ConfigureHttpJsonOptions(o => o.SerializerOptions.PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower);

var app = builder.Build();
app.MapOpenApi();
app.MapScalarApiReference();

var orders = Enumerable.Range(1, 3)
    .Select(i => new Order(i, 100 + i, "open", [new Line($"SKU-{i:000}", i)], null))
    .ToList();

app.MapGet("/orders", (string? status) => orders.Where(o => status is null || o.Status == status));
app.MapGet("/orders/{id:int}", (int id) =>
    orders.FirstOrDefault(o => o.Id == id) is { } order ? Results.Ok(order) : Results.NotFound())
    .Produces<Order>().Produces(404);
app.MapPost("/orders", (OrderIn input) =>
{
    var order = new Order(orders.Max(o => o.Id) + 1, input.CustomerId, "open", input.Lines, input.Note);
    orders.Add(order);
    return Results.Created($"/orders/{order.Id}", order);
}).Produces<Order>(201);

app.Run();

record Line([Required] string Sku, [Required] int Qty);
record OrderIn([Required] int CustomerId, [Required] List<Line> Lines, string? Note);
record Order(int Id, int CustomerId, string Status, List<Line> Lines, string? Note);
