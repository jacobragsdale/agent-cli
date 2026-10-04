"""An airflow DAG file that fails to import."""
import contoso_crm_sdk  # noqa: F401  (not installed: an import error)
from airflow import DAG  # noqa: F401
