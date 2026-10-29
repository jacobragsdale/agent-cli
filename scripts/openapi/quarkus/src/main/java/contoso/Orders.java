package contoso;

import com.fasterxml.jackson.annotation.JsonProperty;
import jakarta.ws.rs.*;
import jakarta.ws.rs.core.MediaType;
import jakarta.ws.rs.core.Response;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.stream.IntStream;
import org.eclipse.microprofile.openapi.annotations.media.Schema;

/** The orders API on Quarkus with smallrye-openapi at its defaults. */
@Path("/orders")
@Produces(MediaType.APPLICATION_JSON)
@Consumes(MediaType.APPLICATION_JSON)
public class Orders {
    public record Line(@Schema(required = true) String sku, @Schema(required = true) int qty) {}

    public record OrderIn(@JsonProperty("customer_id") @Schema(required = true) int customerId,
                          @Schema(required = true) List<Line> lines,
                          String note) {}

    public record Order(int id, @JsonProperty("customer_id") int customerId, String status, List<Line> lines, String note) {}

    private static final List<Order> ORDERS = new CopyOnWriteArrayList<>(IntStream.rangeClosed(1, 3)
            .mapToObj(i -> new Order(i, 100 + i, "open", List.of(new Line("SKU-00" + i, i)), null))
            .toList());

    @GET
    public List<Order> list(@QueryParam("status") String status) {
        return ORDERS.stream().filter(o -> status == null || o.status().equals(status)).toList();
    }

    @GET
    @Path("/{id}")
    public Order get(@PathParam("id") int id) {
        return ORDERS.stream().filter(o -> o.id() == id).findFirst()
                .orElseThrow(() -> new NotFoundException("order " + id + " not found"));
    }

    @POST
    public Response create(OrderIn in) {
        var order = new Order(ORDERS.size() + 1, in.customerId(), "open", in.lines(), in.note());
        ORDERS.add(order);
        return Response.status(201).entity(order).build();
    }
}
