package contoso;

import com.fasterxml.jackson.annotation.JsonProperty;
import io.swagger.v3.oas.annotations.media.Schema;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.stream.IntStream;
import org.springdoc.core.models.GroupedOpenApi;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.context.annotation.Bean;
import org.springframework.http.HttpStatus;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.*;

/**
 * The orders API on Spring Boot 3 with springdoc at its defaults. The two
 * groups make /v3/api-docs/swagger-config list two specs, the way a service
 * with an admin surface often ships.
 */
@SpringBootApplication
@RestController
public class OrdersApplication {
    public record Line(@Schema(requiredMode = Schema.RequiredMode.REQUIRED) String sku,
                       @Schema(requiredMode = Schema.RequiredMode.REQUIRED) int qty) {}

    public record OrderIn(@JsonProperty("customer_id") @Schema(requiredMode = Schema.RequiredMode.REQUIRED) int customerId,
                          @Schema(requiredMode = Schema.RequiredMode.REQUIRED) List<Line> lines,
                          String note) {}

    public record Order(int id, @JsonProperty("customer_id") int customerId, String status, List<Line> lines, String note) {}

    private final List<Order> orders = new CopyOnWriteArrayList<>(IntStream.rangeClosed(1, 3)
            .mapToObj(i -> new Order(i, 100 + i, "open", List.of(new Line("SKU-00" + i, i)), null))
            .toList());

    public static void main(String[] args) {
        SpringApplication.run(OrdersApplication.class, args);
    }

    @Bean
    GroupedOpenApi ordersGroup() {
        return GroupedOpenApi.builder().group("orders").pathsToMatch("/orders/**", "/orders").build();
    }

    @Bean
    GroupedOpenApi adminGroup() {
        return GroupedOpenApi.builder().group("admin").pathsToMatch("/admin/**").build();
    }

    @GetMapping("/orders")
    List<Order> list(@RequestParam(required = false) String status) {
        return orders.stream().filter(o -> status == null || o.status().equals(status)).toList();
    }

    @GetMapping("/orders/{id}")
    ResponseEntity<Order> get(@PathVariable int id) {
        return orders.stream().filter(o -> o.id() == id).findFirst()
                .map(ResponseEntity::ok).orElse(ResponseEntity.notFound().build());
    }

    @PostMapping("/orders")
    @ResponseStatus(HttpStatus.CREATED)
    Order create(@RequestBody OrderIn in) {
        var order = new Order(orders.size() + 1, in.customerId(), "open", in.lines(), in.note());
        orders.add(order);
        return order;
    }

    @PostMapping("/admin/reset")
    Map<String, String> reset() {
        return Map.of("status", "reset");
    }
}
