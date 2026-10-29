FROM mcr.microsoft.com/dotnet/sdk:8.0 AS build
WORKDIR /src
COPY Orders.csproj .
RUN --mount=type=cache,target=/root/.nuget/packages dotnet restore
COPY Program.cs .
RUN --mount=type=cache,target=/root/.nuget/packages dotnet publish -c Release -o /out --no-restore

FROM mcr.microsoft.com/dotnet/aspnet:8.0
WORKDIR /app
COPY --from=build /out .
EXPOSE 8080
ENTRYPOINT ["dotnet", "Orders.dll"]
