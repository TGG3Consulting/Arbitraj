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
              "", "При первом запуске epsilon.trading выключен. Владелец задаёт epsilon.max_age_ms, epsilon.quote_timeout_ms и предел epsilon.gas_budget_native. Оплата нативным ETH и привилегированные операции в данный торговый путь не включены. [Исходник площадки](https://repo.sourcify.dev/8453/0x303ca5c65aabcb1ce242df93f478c41e0e4d2580) · [Симуляции кошелька](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45980111657/job/140441571118)", ""]
    api = json.loads(gzip.decompress((root / "epsilon_openocean_prepaid_base_wallets.json.gz").read_bytes()))
    roles = json.loads((root / "epsilon_openocean_roles.json").read_bytes())
    lines.extend(["#### Публичные заявки OpenOcean V4", "",
                  f"На Base в блоке {api['block']} обычный HTTPS GET quote и swap предоставил исполнимые заявки зарегистрированному в параметрах получателю. Собственный кошелёк исполнил по четыре заявки напрямую через OpenOcean и через Epsilon. При 4 USDC первый вызов оплачивал отдельную площадку LiquidFi. Ключ API для этих запросов не потребовался.", "",
                  "| Вход и сумма, сырые единицы | Чистая выдача напрямую | Газ напрямую | Чистая выдача через Epsilon | Газ через Epsilon |",
                  "|---|---:|---:|---:|---:|"])
    for direct, nested in zip(api["cases"][::2], api["cases"][1::2]):
        if "exact" not in direct or "exact" not in nested:
            continue
        a,b=direct["exact"],nested["exact"]
        lines.append(f"| {direct['src']} · {direct['n']} | {a['received']} | {a['gas']} | {b['received']} | {b['gas']} |")
    lines.extend(["","Сравнение использует конечную сумму своего кошелька и реальный газ подписываемой формы с расходами публикации. Допуск approve требуется заранее. Заявка с просроченным сроком или с минимумом выше выплаты на единицу откатывается. Перед оплатой модуль связывает ERC20 обеих сторон, собственного плательщика, исходную сумму, получателя и официальный вызов swap без частичного расходования. При восстановлении сверяются оба события, изначальный хеш, nonce и реализация прокси на позиции сделки.", "",
                  "Для epsilon.openocean_url укажите https://open-api.openocean.finance/v4/base. Записи для epsilon.openocean_contracts:", ""])
    for k,v in roles.items():
        label={"router":"venue","ocean":"provider"}.get(k,k)
        lines.append(f"- `8453 {label} {v[0]} {v[1]}`")
    funding=json.loads((root / "epsilon_openocean_funding.json").read_bytes())["roles"]
    for key,(address,code) in funding.items():
        lines.append(f"- `8453 {key} {address} {code}`")
    lines.extend([f"- `8453 wallet <адрес модуля своего владельца> {api['walletHash']}`. Адрес подстановки в архиве исключён из первоначальных настроек.", "",
                 "При сбое API или отклонении формы сохраняется штатная котировка исходного DEX. Отдельная API-форма 0x требует учётных данных. [Описание OpenOcean](https://docs.openocean.finance/docs/swap-api/v4) · [Архив фактических оплат](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45988054029)", ""])
    return lines
