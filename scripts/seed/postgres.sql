/*
  Seed data for the sql integration tests -- Postgres dialect, the same
  bench schema mssql.sql and oracle.sql make. Re-runnable: every CREATE is
  guarded and each table's rows are inserted only while it is empty.
*/
\set ON_ERROR_STOP on
set client_min_messages = warning;

create schema if not exists bench;
set search_path = bench;

------------------------------------------------------------------ tables
create table if not exists customers (
    id           int           not null primary key,
    name         varchar(100)  not null,
    email        varchar(200)  not null unique,
    country      char(2)       not null,
    created_at   timestamp     not null,
    credit_limit numeric(12,2) null
);
create table if not exists orders (
    id          int           not null primary key,
    customer_id int           not null references customers(id),
    ordered_at  timestamp     not null,
    status      varchar(20)   not null,
    total       numeric(12,2) not null
);
create table if not exists order_items (
    id         int           not null primary key,
    order_id   int           not null references orders(id),
    sku        varchar(40)   not null,
    qty        int           not null,
    unit_price numeric(10,2) not null
);
create table if not exists events (
    id          bigint       not null primary key,
    customer_id int          not null,
    kind        varchar(30)  not null,
    payload     varchar(200) not null,
    at          timestamp    not null
);
create table if not exists big_text (
    id   int  not null primary key,
    body text null
);
create table if not exists binary_blobs (
    id   int   not null primary key,
    data bytea null
);
create table if not exists all_types (
    id            int not null primary key,
    c_boolean     boolean null,
    c_smallint    smallint null,
    c_integer     integer null,
    c_bigint      bigint null,
    c_numeric     numeric(18,4) null,
    c_real        real null,
    c_double      double precision null,
    c_char        char(5) null,
    c_varchar     varchar(20) null,
    c_text        text null,
    c_date        date null,
    c_time        time null,
    c_timestamp   timestamp null,
    c_timestamptz timestamptz null,
    c_interval    interval null,
    c_uuid        uuid null,
    c_bytea       bytea null,
    c_json        jsonb null,
    c_array       int[] null
);

------------------------------------------------------------------- data
insert into customers (id, name, email, country, created_at, credit_limit)
select i,
       (array['山田太郎', 'Zoë Bauer', 'Ægir Nilsen', '李雷', 'José Álvarez',
              'Björk Þórsdóttir', 'Анна Иванова', 'محمد الفارسي', 'Mary O''Neill',
              'Šimon Novák'])[i % 10 + 1],
       'customer' || i || '@example.com',
       (array['JP', 'DE', 'NO', 'CN', 'ES', 'IS', 'RU', 'SA', 'IE', 'CZ'])[i % 10 + 1],
       timestamp '2024-01-01 08:30:00' + i * interval '1 day',
       case when i % 7 = 0 then null else 100.50 * i end
from generate_series(1, 50) i
where not exists (select 1 from customers);

insert into orders (id, customer_id, ordered_at, status, total)
select i, (i - 1) % 50 + 1,
       timestamp '2024-03-01 09:00:00' + i * interval '1 hour',
       (array['NEW', 'PAID', 'SHIPPED', 'CANCELLED'])[i % 4 + 1],
       3.25 * i
from generate_series(1, 500) i
where not exists (select 1 from orders);

insert into order_items (id, order_id, sku, qty, unit_price)
select i, (i - 1) % 500 + 1, 'SKU-' || lpad(i::text, 5, '0'), i % 5 + 1, 9.99 + i % 20
from generate_series(1, 2000) i
where not exists (select 1 from order_items);

insert into events (id, customer_id, kind, payload, at)
select i, i % 50 + 1,
       (array['login', 'view', 'click', 'purchase', 'logout'])[i % 5 + 1],
       'payload for event ' || i,
       timestamp '2024-01-01 00:00:00' + i * interval '1 second'
from generate_series(1, 1000000) i
where not exists (select 1 from events);

insert into big_text (id, body)
select * from (values (1, 'short body'), (2, null), (3, repeat('Lorem ipsu', 10240))) v
where not exists (select 1 from big_text);

insert into binary_blobs (id, data)
select * from (values (1, '\x0102030405'::bytea), (2, convert_to(repeat('AB', 1000), 'UTF8'))) v
where not exists (select 1 from binary_blobs);

insert into all_types
select * from (values
    (1, true, 32767::smallint, 2147483647, 9223372036854775807, 12345.6789, 1.25::real,
     12345678901.234::double precision, 'abcde', 'varchar value', 'text value',
     date '2024-05-17', time '13:45:30.123456', timestamp '2024-05-17 13:45:30.123456',
     timestamptz '2024-05-17 13:45:30.123456+02', interval '1 day 02:03:04',
     '6f9619ff-8b86-d011-b42d-00c04fc964ff'::uuid, '\x0102030405060708'::bytea,
     '{"a": [1, 2]}'::jsonb, array[1, 2, 3]),
    (2, null, null, null, null, null, null, null, null, null, null, null, null, null, null,
     null, null, null, null, null)) v
where not exists (select 1 from all_types);

--------------------------------------------------- views, routines, sequences
create or replace view v_customer_totals as
select c.id as customer_id,
       c.name,
       count(o.id) as order_count,
       coalesce(sum(o.total), 0) as total_amount
from customers c
left join orders o on o.customer_id = c.id
group by c.id, c.name;

create or replace view v_recent_orders as
select o.id, o.ordered_at, o.status, o.total, c.name as customer_name
from orders o
join customers c on c.id = o.customer_id
order by o.ordered_at desc, o.id desc
limit 100;

create materialized view if not exists mv_daily_orders as
select ordered_at::date as day, count(*) as orders, sum(total) as total
from orders
group by 1;

-- A Postgres procedure returns no rows, so the one that reads is a function.
create or replace function sp_customer_orders(p_customer_id int)
returns table (id int, ordered_at timestamp, status varchar, total numeric)
language sql stable as $$
    select id, ordered_at, status, total
    from bench.orders
    where customer_id = p_customer_id
    order by ordered_at;
$$;

create or replace procedure sp_mark_shipped(p_order_id int)
language plpgsql as $$
begin
    update bench.orders set status = 'SHIPPED' where id = p_order_id;
end;
$$;

create or replace function fn_order_total(p_order_id int)
returns numeric(12,2) language plpgsql stable as $$
begin
    return (select coalesce(sum(qty * unit_price), 0)
            from bench.order_items where order_id = p_order_id);
end;
$$;

create or replace function tvf_orders_by_status(p_status varchar)
returns setof bench.orders language sql stable
begin atomic
    select * from bench.orders where status = p_status;
end;

create sequence if not exists order_seq as bigint start with 501 increment by 1;

do $$
begin
    if not exists (select 1 from pg_roles where rolname = 'bench_reader') then
        create role bench_reader;
    end if;
end
$$;
grant select on customers to bench_reader;
