"""A DAG with params, for run create --conf."""
import pendulum
from airflow import DAG
from airflow.decorators import task
from airflow.models.param import Param

with DAG("e2e_params", start_date=pendulum.datetime(2026, 1, 1), schedule=None, catchup=False,
         tags=["e2e"], params={"day": Param(None, type=["null", "string"], description="The day to load"),
                               "full": Param(False, type="boolean")}) as dag:

    @task
    def show(params=None):
        return {"day": params["day"], "full": params["full"]}

    show()
