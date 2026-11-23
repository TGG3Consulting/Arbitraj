"""Compare SQLite journal cursor queries on a read-only database or a sized CI fixture.

The generated database repeats a normalized journal event; it measures the access
path and does not model the live writer, API serialization or the OS page cache.
"""
import argparse
import json
from pathlib import Path
import sqlite3
import statistics
import tempfile
from time import perf_counter_ns


COLUMNS = "id,at,level,kind,venue,message,data,repeats,last_at"
OPTIONAL = f"SELECT {COLUMNS} FROM journal WHERE (?1 IS NULL OR id < ?1) ORDER BY id DESC LIMIT ?2"
FIRST = f"SELECT {COLUMNS} FROM journal ORDER BY id DESC LIMIT ?1"
NEXT = f"SELECT {COLUMNS} FROM journal WHERE id < ?1 ORDER BY id DESC LIMIT ?2"


def generate(path, rows):
    """Populate a CI fixture with the column layout from storage/db.rs and journal.rs."""
    with sqlite3.connect(path) as db:
        db.execute("CREATE TABLE journal (id INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT NOT NULL, "
                   "level TEXT NOT NULL, kind TEXT NOT NULL, venue TEXT, message TEXT NOT NULL, "
                   "data TEXT, repeats INTEGER, last_at TEXT)")
        db.execute("""WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n < ?)
            INSERT INTO journal(id,at,level,kind,venue,message)
            SELECT n, '2026-11-23T14:53:01.266291220Z','Warn','core.balance_external','poloniex',
            'USDT +70.5413350700 (Poloniex balances eventType=DEPOSIT)' FROM seq""", (rows,))


def compare(path):
    """Check identical rows inside one read transaction and measure SQL + fetch separately from setup."""
    db = sqlite3.connect(path.resolve().as_uri() + "?mode=ro", uri=True)
    try:
        db.execute("BEGIN")
        low, high = db.execute("SELECT min(id), max(id) FROM journal").fetchone()
        if low is None:
            raise ValueError("journal is empty")
        results = []
        for label, cursor in [("first", None), ("middle", (low + high) // 2), ("deep", low + 100), ("empty", low)]:
            alternatives = [(OPTIONAL, (cursor, 80)), (FIRST, (80,)) if cursor is None else (NEXT, (cursor, 80))]
            rows = [db.execute(query, args).fetchall() for query, args in alternatives]
            assert rows[0] == rows[1], label
            times = [[], []]
            for rep in range(21):
                for idx in (range(2) if rep % 2 == 0 else (1, 0)):
                    query, args = alternatives[idx]
                    start = perf_counter_ns()
                    db.execute(query, args).fetchall()
                    times[idx].append((perf_counter_ns() - start) / 1e6)
            for name, (query, args), ms in zip(("optional", "cursor"), alternatives, times):
                plan = [r[3] for r in db.execute("EXPLAIN QUERY PLAN " + query, args)]
                results.append(dict(page=label, query=name, returned=len(rows[0]), plan=plan,
                                    p50_ms=statistics.median(ms), p95_ms=sorted(ms)[19]))
        return dict(sqlite=sqlite3.sqlite_version, id_range=[low, high], results=results)
    finally:
        db.close()


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    group = cli.add_mutually_exclusive_group(required=True)
    group.add_argument("--copy", type=Path)
    group.add_argument("--fixture-rows", type=int, choices=range(1, 10_000_001), metavar="N")
    args = cli.parse_args()
    with tempfile.TemporaryDirectory() as folder:
        path = args.copy or Path(folder) / "sized.sqlite"
        if args.copy is None:
            generate(path, args.fixture_rows)
        result = compare(path)
        result["source"] = "copy" if args.copy else "sized fixture"
        print(json.dumps(result, indent=2))
