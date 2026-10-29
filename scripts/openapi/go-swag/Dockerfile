FROM golang:1.25 AS build
WORKDIR /src
RUN --mount=type=cache,target=/go/pkg/mod go install github.com/swaggo/swag/cmd/swag@v1.16.6
COPY go.mod main.go ./
# go.mod pins the direct modules; tidy resolves the rest from their go.mod
# files, which is deterministic, so no go.sum is kept.
RUN --mount=type=cache,target=/go/pkg/mod --mount=type=cache,target=/root/.cache/go-build \
    swag init && go mod tidy && CGO_ENABLED=0 go build -o /orders .

FROM debian:bookworm-slim
COPY --from=build /orders /orders
ENV GIN_MODE=release
EXPOSE 8080
ENTRYPOINT ["/orders"]
