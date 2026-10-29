# The rich orders API: fastapi, fastapi-v2 and echo all run this image.
FROM python:3.12-slim
RUN pip install --no-cache-dir fastapi==0.162.0 uvicorn==0.54.0 pydantic==2.14.1
WORKDIR /app
COPY app.py .
EXPOSE 8000
CMD ["uvicorn", "app:app", "--host", "0.0.0.0", "--port", "8000", "--no-access-log"]
