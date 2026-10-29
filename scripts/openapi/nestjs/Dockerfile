FROM node:22-slim
WORKDIR /app
COPY package.json .
RUN --mount=type=cache,target=/root/.npm npm install --no-audit --no-fund
COPY tsconfig.json .
COPY src src
RUN npm run build
EXPOSE 3000
CMD ["node", "dist/main.js"]
