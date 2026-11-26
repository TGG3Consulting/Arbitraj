"""Capture Relayscan's nominal windows and compare aligned completed UTC days in DefiLlama."""
from datetime import datetime, timezone, timedelta
import json
import re

import requests
from bs4 import BeautifulSoup

from record_fermi_ethereum import ROOT

DEST=ROOT/"tests/data/first_run_dex_2026-11-26/fermi_market_windows.json"
def published(window):
    url="https://www.relayscan.io/overview?t="+window
    page=requests.get(url,timeout=20);page.raise_for_status()
    html=BeautifulSoup(page.text,"lxml")
    slot=int(re.search(r"Updated at slot\s+([0-9]+)",html.get_text(" ",strip=True)).group(1))
    table=next(t for t in html.select("table") if not t.find("table") and "Builder (extra_data)" in t.select_one("tr").get_text())
    rows=[]
    for tr in table.select("tbody#tbody-builders-all > tr.tr-builder-parent"):
        cells=[cell.get_text(" ",strip=True) for cell in tr.find_all(["td","th"],recursive=False)]
        if len(cells)>=3: rows.append(dict(label=cells[0],blocks=int(cells[1].replace(",","")),percent=cells[2]))
    total=sum(x["blocks"] for x in rows)
    selected={x["label"]:x for x in rows if x["label"] in {"Titan (titanbuilder.xyz)","BuilderNet"}}
    if len(selected)!=2 or not total:raise ValueError("builder table incomplete")
    for r in selected.values():
        if abs(100*r["blocks"]/total - float(r["percent"].replace("%","").strip()))>0.05:
            raise ValueError("published denominator differs")
    return dict(source=url,as_of_slot=slot,as_of_utc=datetime.fromtimestamp(1606824023+12*slot,timezone.utc).isoformat(),
                methodology="builder table grouped by attributed extra_data, not unique relay reports or all network slots",
                table_blocks=total,selected=selected,all_rows=rows)


def main():
    urls={"ethereum":"https://api.llama.fi/overview/dexs/Ethereum?excludeTotalDataChart=false&excludeTotalDataChartBreakdown=true",
          "fermi":"https://api.llama.fi/summary/dexs/fermiswap?excludeTotalDataChart=false&excludeTotalDataChartBreakdown=true"}
    daily={}
    for k,url in urls.items():
        r=requests.get(url,timeout=20);r.raise_for_status();d=r.json()
        if k=="fermi" and d.get("chains")!=["Ethereum"]:raise ValueError("Fermi not single-chain")
        daily[k]={datetime.fromtimestamp(stamp,timezone.utc).date():amount for stamp,amount in d["totalDataChart"]}
    end=datetime(2026,11,25,tzinfo=timezone.utc).date()
    days=[end-timedelta(days=i) for i in reversed(range(7))]
    series=[dict(utc=str(day),ethereum_usd=daily["ethereum"][day],fermi_usd=daily["fermi"][day]) for day in days]
    out=dict(retrieved_utc=datetime.now(timezone.utc).isoformat(),builders={t:published(t) for t in ["24h","7d"]},
             volume_sources=urls,volume_days=series,volume={})
    for window, wanted in [("1_closed_utc_day",series[-1:]),("7_closed_utc_days",series)]:
        market=sum(x["ethereum_usd"] for x in wanted);venue=sum(x["fermi_usd"] for x in wanted)
        out["volume"][window]=dict(ethereum_usd=market,fermi_usd=venue,percent=100*venue/market)
    DEST.write_text(json.dumps(out,indent=2,ensure_ascii=False)+"\n",encoding="utf8")
    print([(n,v["as_of_slot"],v["table_blocks"],v["selected"]) for n,v in out["builders"].items()],out["volume"])


if __name__=="__main__":
    main()
