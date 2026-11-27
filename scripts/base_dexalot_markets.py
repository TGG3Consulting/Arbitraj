"""Operator settings and paid Base evidence for Dexalot Simple Swap."""

import gzip
import json
import tomllib
from decimal import Decimal
from pathlib import Path


def settings():
    data = Path("tests/data/first_run_dex_2026-11-26")
    registry = json.loads(gzip.decompress((data / "dexalot_base_registry.json.gz").read_bytes()))
    wallet = json.loads(gzip.decompress((data / "dexalot_base_wallet.json.gz").read_bytes()))
    market = json.loads((data / "dexalot_base_indicative.json").read_bytes())
    first_run = tomllib.loads(Path("config/first_run.toml").read_text(encoding="utf8"))
    defaults = {r["name"]: r["value"] for s in first_run["section"] for r in s.get("rec", [])
                if r["name"].startswith("dexalot.") and r.get("prefill", {}).get("id") == "base_dex"}
    lines = ["### Dexalot на Base", "",
             "Площадка `base_dex`, сеть 8453. В Первом запуске сохраните URL и строки контрактов. "
             "Разверните модуль Dexalot из карточки своего кошелька. После подтверждения в сети "
             "интерфейс добавляет адрес и хеш модуля в настройки. Адрес подписанта и получателя "
             "должны соответствовать роли в индивидуальной заявке.", "",
             "| Настройка | Значение для Base |", "|---|---|"]
    for key in ("dexalot.base_url", "dexalot.roles"):
        values = "<br>".join(f"`{v}`" for v in defaults[key].split(","))
        lines.append(f"| `{key}` | {values} |")
    lines.extend(["| `dexalot.api_key` | Секрет. Ключ канала Dexalot для заголовка x-apikey. |",
                  "| `dexalot.trading` | По умолчанию выключено. Включает участие подписанных заявок в выборе маршрута. |",
                  "| `dexalot.min_validity_ms` | Положительный остаток срока заявки в миллисекундах. Величину задаёт владелец. |",
                  "| `dexalot.gas_budget_native` | Положительный лимит расходов в ETH на один обмен. Величину задаёт владелец. |", "",
                  "Для индивидуальных котировок подайте заявку команде Dexalot на RFQ Simple Swap для Base. "
                  "В заявке укажите сеть и адрес своего модуля для допуска исполнителя. "
                  "[Описание доступа](https://docs.dexalot.com/en/apiv2/SimpleSwap.html) · "
                  "[Контакты](https://docs.dexalot.com/en/Contact.html).", "",
                  "Публичный ответ pairs перечислил " + ", ".join(market["pairs"]) + ". "
                  "Цена pairprice служит предварительной оценкой. Исполнимая сумма определяется подписанной "
                  "заявкой на свой кошелёк и фактической выплатой после корректировки маркетмейкера.", ""])
    day = registry["interval"]
    events = day["cash_events"]
    lines.extend([f"За {(day['end_time'] - day['start_time']) // 3600} ч, блоки {day['first']}–{day['last']}, "
                  f"в исходном списке {len(registry['members'])} разрешённых кассы. "
                  f"Получено {len(events)} квитанций SwapExecuted. Это операции других участников сети.", "",
                  "| Блок | Контракт кассы | Списано, минимальные единицы токена | Выдано, минимальные единицы токена |",
                  "|---:|---|---|---|"])
    for event in events:
        target = "0x" + event["target"][-40:]
        lines.append(f"| [{event['block']}](https://basescan.org/tx/{event['tx']}) | `{event['maker']}` | "
                     f"{event['input']} · `{event['source']}` | {event['output']} · `{target}` |")
    lines.extend(["", "Отдельный архивный повтор исходной подписи USDC → WETH через модуль кошелька.", "",
                  "| Режим | Списано USDC | Выход в подписи, WETH | Зачислено WETH | Газ, единицы |", "|---|---:|---:|---:|---:|"])
    for row in wallet["controls"]:
        units = lambda key, power: format(Decimal(row[key]).scaleb(-power), "f")
        mode = "Часть" if row["partial"] else "Целиком"
        lines.append(f"| {mode} | {units('spent', 6)} | {units('nominal', 18)} | {units('net', 18)} | {row['gas']} |")
    lines.extend(["", "Минимум выше фактической выплаты на одну минимальную единицу WETH вызывает откат. "
                  "Проверены повтор заявки, истечение срока и чужой отправитель. "
                  "[Повтор выплат](https://github.com/TGG3Consulting/Arbitraj/actions/runs/46028860696) · "
                  "[Реестр и суточные события](https://github.com/TGG3Consulting/Arbitraj/actions/runs/46029436907).", ""])
    return lines
