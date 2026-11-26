"""Measure the cash-hold index migration on generated historical ledger rows in Actions."""
import json
import sqlite3
import statistics
import sys
import tempfile
from pathlib import Path
from time import perf_counter_ns


def timed(work):
    start = perf_counter_ns()
    result = work()
    return result, (perf_counter_ns() - start) / 1_000_000


def measure(db, predicate):
    create = f"CREATE INDEX IF NOT EXISTS cex_funds_held_deals ON core_state(id) WHERE {predicate}"
    query = f"SELECT id FROM core_state INDEXED BY cex_funds_held_deals WHERE {predicate}"
    db.execute("CREATE TABLE core_state(kind TEXT,id TEXT,body TEXT,updated_at TEXT,PRIMARY KEY(kind,id))")
    insert = "INSERT INTO core_state VALUES(?,?,?,?)"
    n = 25000
    db.executemany(insert, (("deal", f"closed-{i}", '{"step":"Closed","usdt_return_parts":[]}', "") for i in range(n)))
    expected = {"malformed"}
    db.execute(insert, ("deal", "malformed", "{", ""))
    def sample(label, value, selected):
        db.execute(insert, ("deal", label, json.dumps(value), ""))
        if selected:
            expected.add(label)
    for state in ("pending", "not_returned", "retrying", "done", "manual"):
        selected = state in {"pending", "not_returned", "retrying"}
        sample("legacy-" + state, {"usdt_return": {"state": state}}, selected)
        sample("part-" + state, {"usdt_return_parts": [{"state":"done"}, {"state":state}]}, selected)
    for step in ("Buying", "Transfer", "Selling", "Stuck", "WaitingHuman", "Closed", "Halted", "TakenOver"):
        sample("mixed-" + step, {"step":step, "plan":{"settlement_cycle":{}}}, step not in {"Closed","Halted","TakenOver"})
    for step, buy, selected in (("Buying",None,True),("Halted",None,True),("Closed",None,False),("Halted",{},False)):
        sample("before-native-" + str((step,buy)), {"step":step,"buy":buy,"plan":{"size":{"budget":"80"}}}, selected)
    for group, selected in (({},False), ({"wait":None},False), ({"wait":["venue","0"]},True), ({"wait":["venue","24"]},True)):
        sample("group-" + str(group), {"step":"Closed","group":group}, selected)
    db.execute(insert, ("other","different-kind",'{"usdt_return":{"state":"pending"}}',""))
    db.commit()
    _, first_ms = timed(lambda: db.execute(create))
    _, second_ms = timed(lambda: db.execute(create))
    def scan():
        return {row[0] for row in db.execute(query)}
    samples = [timed(scan) for _ in range(100)]
    assert all(rows == expected for rows, _ in samples), samples[0][0] ^ expected
    plan = [row[3] for row in db.execute("EXPLAIN QUERY PLAN " + query)]
    assert any("cex_funds_held_deals" in row for row in plan), plan
    return {"sqlite":sqlite3.sqlite_version, "generated_closed":n, "selected":len(expected),
            "initial_index_ms":first_ms, "repeated_open_index_ms":second_ms,
            "median_scan_ms":statistics.median(ms for _, ms in samples),"query_plan":plan}


if __name__ == "__main__":
    predicate = Path("src/storage/cex_funds_held.sql").read_text(encoding="utf-8").strip()
    with tempfile.TemporaryDirectory() as directory:
        with sqlite3.connect(Path(directory) / "ledger.sqlite") as db:
            report = measure(db, predicate)
        db.close()
    Path(sys.argv[1]).write_text(json.dumps(report, indent=2), encoding="utf-8")
