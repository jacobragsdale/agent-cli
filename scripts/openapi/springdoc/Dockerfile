FROM maven:3.9-eclipse-temurin-21 AS build
WORKDIR /src
COPY pom.xml .
RUN --mount=type=cache,target=/root/.m2 mvn -B -q dependency:go-offline
COPY src src
RUN --mount=type=cache,target=/root/.m2 mvn -B -q -DskipTests package && cp target/orders-1.0.0.jar /orders.jar

FROM eclipse-temurin:21-jre
COPY --from=build /orders.jar /orders.jar
EXPOSE 8080
ENTRYPOINT ["java", "-jar", "/orders.jar"]
