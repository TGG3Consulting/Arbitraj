"""Готовит таблицу сторон Epsilon по каноническим квитанциям и отдельным симуляциям."""
import gzip
import json
from pathlib import Path


def epsilon_markets():
    root = Path("tests/data/first_run_dex_2026-11-26")
    data = json.loads(gzip.decompress((root / "epsilon_base_wallets.json.gz").read_bytes()))
    receipts = json.loads(gzip.decompress((root / "epsilon_base_transactions.json.gz").read_bytes()))
    recorded = data["cases"]
    lines = ["### AlienBase Epsilon на Base", "",
             f"Блок {data['height']}. За 43 200 блоков перед ним подтверждены {sum('Swapped' in t['events'] for t in receipts)} публичных свопов и {sum('OrderFilled' in t['events'] for t in receipts)} исполнений поручений. В четырёх прямых вызовах swap один пользовательский вход проходит через OpenOcean или 0x. Пятый своп вызван вложенно. Владельцу отдельного EOA доступен swap с собственными средствами. Для DCA, лимитных и trailing-поручений действуют права keeper, для matcherPullLeg и matcherExecuteHybridLeg нужен принятый контракт matcher.", "",
             "В боте сравниваются собственные ERC20, два разрешённых V3 роутера и смешанные доли двух разных пулов. Каждая доля получает свою цену с учётом комиссии пула. Epsilon удерживает ставку protocolFeeBps с полного входа до разделения. При 5 bps в этом блоке это floor(amount × 5 / 10 000). Сбор входит в расход токена. Газ и публикация в L1 входят отдельно. Отдельный прямой пул сохраняет собственную котировку и общую с источником ликвидность.", "",
             "| Способ | Число выплат | Газ обмена | Газ исходного approve |",
             "|---|---:|---:|---:|"]
    for kind, label in [("provider", "Непосредственно роутеру провайдера"), ("epsilon", "Собственный EOA через Epsilon"), ("guarded", "Через EpsilonWallet с проверкой конечного баланса")]:
        subset = [r for r in recorded if r["mode"] == kind]
        def bounds(k):
            vals = [r[k] for r in subset]
            return f"{min(vals)}–{max(vals)}"
        lines.append(f"| {label} | {len(subset)} | {bounds('gas')} | {bounds('approveGas')} |")
    lines += ["", "WETH и USDC проверены в обе стороны на двух размерах. Лучший прямой провайдер в каждой из четырёх групп дал больше токенов при меньшем газе. На каждом из 32 путей минимум выше выплаты на одну исходную единицу и истёкший срок привели к откату. Для прямой оплаты обе валюты должны пройти действующий профиль точных ERC20 для каждого участвующего провайдера. Для защищённой формы нужен собственный модуль владельца. Обе формы используют выданное им разрешение. Штатное восстановление проверяет исходную сеть, хеш, подписанта, nonce, позицию, события участвовавших пулов и чистую выплату.", "",
              "Настройка площадки epsilon.contracts принимает строки ниже, где family заменяется действующим ID этой DEX из настроек. Идентификаторы первого запуска приведены для соответствующих фабрик. Порядок двух записей provider соответствует привязке первого и второго провайдера в защитном модуле.", "",
              f"- `8453 venue {data['router']} {data['routerHash']}`"]
    for uri, family in zip(data["providers"], ["alien-base-v3", "uniswap-v3-base"]):
        lines.append(f"- `8453 provider {family} {uri['router']} 0x{uri['code']}`")
    lines += [f"- Необязательная запись `8453 wallet <адрес размещённого владельцем модуля> {data['walletHash']}`. Архивная проверка использует адрес изолированной симуляции.",
              "", "При первом запуске epsilon.trading выключен. Владелец задаёт epsilon.max_age_ms, epsilon.quote_timeout_ms и предел epsilon.gas_budget_native. Оплата нативным ETH, маршруты агрегаторов с внешней заявкой и привилегированные операции в данный торговый путь не включены. [Исходник площадки](https://repo.sourcify.dev/8453/0x303ca5c65aabcb1ce242df93f478c41e0e4d2580) · [Симуляции кошелька](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45980111657/job/140441571118)", ""]
    return lines
