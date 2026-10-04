"""A sensor deferred to the triggerer until 2099."""
import pendulum
from airflow import DAG

try:
    from airflow.providers.standard.sensors.date_time import DateTimeSensorAsync
except ImportError:
    from airflow.sensors.date_time import DateTimeSensorAsync

with DAG("e2e_deferred", start_date=pendulum.datetime(2026, 1, 1), schedule=None, catchup=False,
         tags=["e2e"]) as dag:
    DateTimeSensorAsync(task_id="wait_for_2099", target_time=pendulum.datetime(2099, 1, 1))
