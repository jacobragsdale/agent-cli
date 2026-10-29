FROM python:3.12-slim
RUN pip install --no-cache-dir Django==5.2.18 djangorestframework==3.18.3 drf-spectacular==0.30.0
WORKDIR /app
COPY app.py .
EXPOSE 8000
CMD ["python", "app.py", "runserver", "0.0.0.0:8000", "--noreload"]
