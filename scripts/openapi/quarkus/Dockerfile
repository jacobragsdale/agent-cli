FROM maven:3.9-eclipse-temurin-21 AS build
WORKDIR /src
COPY pom.xml .
RUN --mount=type=cache,target=/root/.m2 mvn -B -q dependency:go-offline
COPY src src
RUN --mount=type=cache,target=/root/.m2 mvn -B -q -DskipTests package && cp -r target/quarkus-app /quarkus-app

FROM eclipse-temurin:21-jre
COPY --from=build /quarkus-app /quarkus-app
EXPOSE 8080
ENTRYPOINT ["java", "-jar", "/quarkus-app/quarkus-run.jar"]
