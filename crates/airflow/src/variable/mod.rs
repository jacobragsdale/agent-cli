//! `airflow variable`: Airflow Variables, by key. Never their values: some
//! hold secrets, so no row type has a field one could go in.

pub(crate) mod list;
