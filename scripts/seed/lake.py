#!/usr/bin/env python3
"""Write the contoso world's Data Lake: its Parquet files and its recordings.

The files go under fixtures/world/files/lake/ACCOUNT/CONTAINER/PATH, written
by DuckDB; fixtures/world/http/lake.json answers every Path List (one level
and recursive), Get Properties and Get Access Control HEAD, and ranged read
of them, so a listing's sizes always match the files. Run it after changing
the tree below, then commit both:

  uv run --no-project --python 3.12 --with duckdb==1.4.1 scripts/seed/lake.py

The facts it holds are in fixtures/world/facts/lake.md.
"""

import json
import pathlib
import shutil
from datetime import datetime, timezone
from email.utils import format_datetime
from urllib.parse import quote

import duckdb

ROOT = pathlib.Path(__file__).resolve().parents[2] / "fixtures" / "world"
FILES = ROOT / "files" / "lake"

AIRFLOW = "00000000-0000-4000-8000-00000000a1f2"  # id-contoso-airflow's principal
FINANCE = "00000000-0000-4000-8000-00000000a2f2"  # the finance close's principal

ROOT_ACL = "user::rwx,group::r-x,other::--x"
SALES_DIR = f"user::rwx,group::r-x,user:{AIRFLOW}:rwx,mask::rwx,other::---"
SALES_FILE = f"user::rw-,group::r--,user:{AIRFLOW}:rw-,mask::rw-,other::---"
FINANCE_DIR = f"user::rwx,group::r-x,user:{AIRFLOW}:r-x,mask::r-x,other::---"
LEDGER_DIR = f"user::rwx,group::r-x,user:{FINANCE}:rwx,mask::rwx,other::---"
LEDGER_FILE = f"user::rw-,group::r--,user:{FINANCE}:rw-,mask::rw-,other::---"

ORDERS = """SELECT order_id::BIGINT AS order_id, customer_id::BIGINT AS customer_id,
  total::DOUBLE AS total, placed_at::TIMESTAMPTZ AS placed_at FROM (VALUES {rows})
  t(order_id, customer_id, total, placed_at)"""


def orders(day, rows):
    values = ", ".join(
        f"({o}, {'NULL' if c is None else c}, {t}, TIMESTAMPTZ '2026-09-{day:02d} {h}+00')"
        for o, c, t, h in rows
    )
    return ORDERS.format(rows=values)


# Each folder: (path, owner, acl, written), each file the same plus its SQL.
FOLDERS = [
    ("contosolake", "curated", "", "$superuser", ROOT_ACL, "2026-09-29T02:15:03"),
    ("contosolake", "curated", "finance", "$superuser", FINANCE_DIR, "2026-09-29T02:15:03"),
    ("contosolake", "curated", "finance/ledger", FINANCE, LEDGER_DIR, "2026-09-29T02:15:03"),
    ("contosolake", "curated", "sales", "$superuser", SALES_DIR, "2026-09-27T00:38:10"),
    ("contosolake", "curated", "sales/orders_daily", AIRFLOW, SALES_DIR, "2026-09-27T00:38:10"),
    ("contosolake", "raw", "", "$superuser", ROOT_ACL, "2026-09-29T06:02:11"),
    ("contosolake", "raw", "finance", "$superuser", FINANCE_DIR, "2026-09-28T23:50:41"),
    ("contosolake", "raw", "finance/journal", FINANCE, LEDGER_DIR, "2026-09-28T23:50:41"),
    ("contosolake", "raw", "sales", "$superuser", SALES_DIR, "2026-09-29T06:02:11"),
    ("contosolake", "raw", "sales/orders", AIRFLOW, SALES_DIR, "2026-09-29T06:02:11"),
    ("contosolakedev", "scratch", "", "$superuser", ROOT_ACL, "2026-09-02T09:30:00"),
]
PARQUET = [
    ("contosolake", "curated", "finance/ledger/part-0000.parquet", FINANCE, LEDGER_FILE, "2026-09-29T02:15:03",
     """SELECT range + 1 AS entry_id, ['4000-revenue', '5000-cogs', '6100-payroll', '1200-receivables'][range % 4 + 1] AS account,
       ((range * 37) % 5000 + 0.25)::DECIMAL(12,2) AS amount, DATE '2026-09-01' + (range % 28)::INTEGER AS posted
       FROM range(120)"""),
    ("contosolake", "curated", "sales/orders_daily/part-0000.parquet", AIRFLOW, SALES_FILE, "2026-09-27T00:38:10",
     """SELECT * FROM (VALUES (DATE '2026-09-23', 5, 431.25), (DATE '2026-09-24', 4, 288.0),
       (DATE '2026-09-25', 6, 517.4), (DATE '2026-09-26', 6, 602.15)) t(date, orders, revenue)"""),
    ("contosolake", "raw", "finance/journal/part-0000.parquet", FINANCE, LEDGER_FILE, "2026-09-28T23:50:41",
     """SELECT range + 1 AS line, 'JE-' || (900 + range // 4) AS journal, ((range * 53) % 900 + 0.5)::DECIMAL(12,2) AS amount
       FROM range(20)"""),
]
DAYS = {
    23: [(88100, 5512, 129.9, "08:14:02"), (88101, 4411, 54.0, "09:40:11"), (88102, 6020, 99.0, "12:01:45"),
         (88103, 3307, 18.35, "15:22:09"), (88104, 5512, 130.0, "19:48:30")],
    24: [(88105, 4411, 72.0, "07:55:12"), (88106, 6020, 81.5, "11:12:40"), (88107, 2210, 60.0, "16:30:00"),
         (88108, 3307, 74.5, "20:05:19")],
    25: [(88109, 5512, 99.9, "08:01:00"), (88110, 2210, 102.0, "09:15:30"), (88111, 4411, 64.5, "10:44:12"),
         (88112, 6020, 88.0, "13:20:41"), (88113, 3307, 41.0, "17:03:59"), (88114, 5512, 122.0, "21:37:06")],
    26: [(88115, 2210, 150.0, "08:30:00"), (88116, 6020, 99.15, "10:02:11"), (88117, 4411, 75.0, "12:12:12"),
         (88118, 3307, 58.0, "14:45:00"), (88119, 5512, 110.0, "18:20:30"), (88120, 2210, 110.0, "22:59:01")],
    28: [(88121, 5512, 129.9, "09:02:17"), (88122, 4411, 54.0, "11:30:00"), (88123, None, 18.5, "14:11:52"),
         (88124, 6020, 302.75, "20:48:05")],
    29: [(88125, 2210, 45.0, "00:41:10"), (88126, 5512, 88.2, "01:15:00"), (88127, None, 12.0, "02:02:02"),
         (88128, 6020, 230.0, "03:30:45"), (88129, 4411, 61.5, "04:12:19"), (88130, None, 19.99, "05:01:33"),
         (88131, 3307, 140.0, "05:48:00")],
}
LANDED = {23: "2026-09-23T06:01:58", 24: "2026-09-24T06:02:03", 25: "2026-09-25T06:01:49",
          26: "2026-09-26T06:02:20", 28: "2026-09-28T06:02:09", 29: "2026-09-29T06:02:11"}
for day, rows in DAYS.items():
    path = f"sales/orders/date=2026-09-{day:02d}"
    FOLDERS.append(("contosolake", "raw", path, AIRFLOW, SALES_DIR, LANDED[day]))
    PARQUET.append(("contosolake", "raw", f"{path}/part-0001.parquet", AIRFLOW, SALES_FILE, LANDED[day], orders(day, rows)))

ETAGS = iter(range(0x8DDFF1A00000001, 0x8DDFF1B00000000, 0x1D))


def http_time(stamp):
    return format_datetime(datetime.fromisoformat(stamp).replace(tzinfo=timezone.utc), usegmt=True)


def permissions(acl):
    bits = dict(entry.rsplit(":", 1) for entry in acl.split(",") if entry.count(":") == 2)
    named = any(not entry.startswith(("user::", "group::", "mask::", "other::")) for entry in acl.split(","))
    group = bits.get("mask:", bits.get("group:"))
    return bits["user:"] + group + bits["other:"] + ("+" if named else "")


def url(account, container, path, slash=False):
    base = f"https://{account}.dfs.core.windows.net/{container}"
    if path:
        return f"{base}/{quote(path, safe='/-_.~')}"
    return base + ("/" if slash else "")


def main():
    shutil.rmtree(FILES, ignore_errors=True)
    entries = []
    for account, container, path, owner, acl, written in FOLDERS:
        entries.append(dict(account=account, container=container, path=path, owner=owner, acl=acl,
                            written=written, size=0, directory=True, etag=f"0x{next(ETAGS):X}"))
    db = duckdb.connect()
    for account, container, path, owner, acl, written, sql in PARQUET:
        local = FILES / account / container / path
        local.parent.mkdir(parents=True, exist_ok=True)
        db.execute(f"COPY ({sql}) TO '{local}' (FORMAT parquet)")
        entries.append(dict(account=account, container=container, path=path, owner=owner, acl=acl,
                            written=written, size=local.stat().st_size, directory=False,
                            etag=f"0x{next(ETAGS):X}", file=str(local.relative_to(ROOT))))

    def listed(entry):
        held = {"name": entry["path"], "contentLength": str(entry["size"]), "etag": entry["etag"],
                "lastModified": http_time(entry["written"]), "owner": entry["owner"], "group": "$superuser",
                "permissions": permissions(entry["acl"])}
        if entry["directory"]:
            held["isDirectory"] = "true"
        return held

    exchanges = []
    accounts = sorted({entry["account"] for entry in entries})
    for account in accounts:
        roots = [e for e in entries if e["account"] == account and e["path"] == ""]
        exchanges.append({
            "request": f"GET https://{account}.dfs.core.windows.net/?resource=account",
            "answer": {"filesystems": [{"name": e["container"], "lastModified": http_time(e["written"]),
                                        "etag": e["etag"]} for e in roots]}})
        exchanges.append({
            "request": f"GET https://{account}.dfs.core.windows.net/?resource=account&maxResults=1",
            "note": "doctor lake",
            "answer": {"filesystems": [{"name": roots[0]["container"], "lastModified": http_time(roots[0]["written"]),
                                        "etag": roots[0]["etag"]}]}})
    for folder in [e for e in entries if e["directory"]]:
        same = [e for e in entries if e["account"] == folder["account"] and e["container"] == folder["container"]
                and e["path"] != folder["path"]]
        prefix = folder["path"] + "/" if folder["path"] else ""
        below = sorted((e for e in same if e["path"].startswith(prefix)), key=lambda e: e["path"])
        for recursive in (False, True):
            kept = [e for e in below if recursive or "/" not in e["path"][len(prefix):]]
            query = f"resource=filesystem&recursive={'true' if recursive else 'false'}&maxResults=5000"
            if folder["path"]:
                query += "&directory=" + quote(folder["path"], safe="-_.~")
            exchanges.append({
                "request": f"GET https://{folder['account']}.dfs.core.windows.net/{folder['container']}?{query}",
                "answer": {"paths": [listed(e) for e in kept]}})
    for entry in entries:
        target = url(entry["account"], entry["container"], entry["path"], slash=True)
        properties = {"Last-Modified": http_time(entry["written"]), "ETag": f'"{entry["etag"]}"',
                      "x-ms-resource-type": "directory" if entry["directory"] else "file",
                      "Content-Length": str(entry["size"])}
        access = {"x-ms-owner": entry["owner"], "x-ms-group": "$superuser",
                  "x-ms-permissions": permissions(entry["acl"]), "x-ms-acl": entry["acl"]}
        exchanges.append({"request": f"HEAD {target}", "headers": properties, "text": ""})
        for upn in ("&upn=true", ""):
            exchanges.append({"request": f"HEAD {target}?action=getAccessControl{upn}", "headers": access, "text": ""})
        if not entry["directory"]:
            exchanges.append({"request": f"GET {target}", "body_file": entry["file"],
                              "headers": {"Content-Type": "application/octet-stream"}})
    exchanges.append({
        "request": f"POST https://graph.microsoft.com/v1.0/directoryObjects/{AIRFLOW}/getMemberObjects",
        "note": "lake access get: id-contoso-airflow is in no group",
        "answer": {"@odata.context": "https://graph.microsoft.com/v1.0/$metadata#Collection(Edm.String)", "value": []}})
    text = json.dumps(exchanges, indent=1)
    (ROOT / "http" / "lake.json").write_text(text + "\n")
    print(f"{len(exchanges)} exchanges, {sum(1 for e in entries if not e['directory'])} files")


if __name__ == "__main__":
    main()
