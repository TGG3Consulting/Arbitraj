"""Сопоставляет зарегистрированные рынки 2THICK и оплаченные формы на одном каноническом блоке."""
import gzip
import json
from pathlib import Path


def spacing_markets():
    root = Path("tests/data/first_run_dex_2026-11-26")
    archive = json.loads(gzip.decompress((root / "wagmi_thick_base.json.gz").read_bytes()))["2thick"]
    venice = json.loads(gzip.decompress((root / "2thick_venice_base_wallets.json.gz").read_bytes()))
    assert archive["selector"] == venice["selector"]
    markets = archive["markets"]
    changed = sum(m["initial_fee"] != m["current_fee"] for m in markets)
    lines = ["### 2THICK на Base", "",
             f"В блоке {archive['height']} зарегистрировано {len(markets)} рынков. Положительная ликвидность на текущем тике есть у {sum(m['liquidity']>0 for m in markets)}. У {changed} ставка пула отличается от 10 000 пипсов при создании. Она может меняться без события. Профиль 2thick-base использует шаги 1, 8, 64 для поиска, читает fee() каждого подписанного пула пачкой на блок и проверяет её перед оплатой.", "",
             "| Проверенные пары | Сопоставленных объёмов и направлений | Разница газа обмена Router02 минус V5 |",
             "|---|---:|---:|"]
    for name, cases in [("WETH и три токена", archive["cases"]), ("VVV и DIEM", venice["cases"])]:
        by_key = {}
        for c in cases:
            by_key.setdefault((c["pool"], c["input"], c["spent"]), {})[c["wire"]] = c
        spread = []
        for forms in by_key.values():
            r, v = forms["router02"], forms["v5"]
            assert r["received"] == v["received"]
            spread.append(r["gas"] - v["gas"])
        lines.append(f"| {name} | {len(spread)} | {min(spread)}–{max(spread)} |")
    lines.extend(["", "Стандартный V5 выполняет шаг 1 и проверяет итоговый приход. Для Router02 задайте dex.v3_wallet_router_abi=router02_spacing и проверенные прямые реализации обоих ERC20 в dex.router_exact_tokens. Первоначальные значения включают WETH, VVV и DIEM. У трёх исследованных пар к WETH второй актив допускает обновление реализации или изменение налога. Для VVV/DIEM в архиве V5 потребовал на 12 gas больше на первоначальный approve. Подготовка VVV отдельным обменом за собственный WETH учтена отдельно от обмена 2THICK. Рейтинг перед подписью использует текущие чистую выдачу, газ, стоимость публикации и требуемый approve.", "",
                  "[Фабрика 2THICK](https://repo.sourcify.dev/8453/0x7ca1dccfb4f49564b8f13e18a67747fd428f1c40) · [Контракт VVV](https://repo.sourcify.dev/8453/0xacfe6019ed1a7dc6f7b508c02d1b04ec88cc21bf) · [Контракт DIEM](https://repo.sourcify.dev/8453/0xf4d97f2da56e8c3098f3a8d538db630a2606a024) · [32 кошельковые выплаты](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45991020807)", ""])
    return lines


def virtus_cl_markets():
    path = Path("tests/data/first_run_dex_2026-11-26/virtus_cl_base.json.gz")
    book = json.loads(gzip.decompress(path.read_bytes()))
    markets = book["markets"]
    matched = {}
    for case in book["cases"]:
        key = (case["pool"], case["input"], case["spent"])
        matched.setdefault(key, {})[case["wire"]] = case
    savings = []
    for forms in matched.values():
        router, v5 = forms["slipstream"], forms["v5"]
        assert router["received"] == v5["received"]
        savings.append(v5["gas"] - router["gas"])
    overrides = sum(m["current_fee"] != m["base_fee"] for m in markets)
    return ["### Virtus CL на Base", "",
            f"На блоке {book['height']} фабрика подтвердила {len(markets)} пулов по обоим токенам и шагу тика. У {overrides} комиссия отличается от базовой ставки для шага. Профиль virtus-cl-base считывает fee() у подписанных пулов каждым блоком и перед подписанием сверяет действующий модуль платы, квотер, фабрику и реализацию клона.", "",
            f"Проверено {len(matched)} сочетаний пула, размера и направления. В каждом кошелёк оплатил одинаковый вход и получил одинаковый выход через Router и V5. Штатный Router с формой classic_spacing сэкономил от {min(savings)} до {max(savings)} единиц газа до публикации Base. Время истечения указано внутри exactInputSingle. Прямой путь по начальному списку доступен для WETH и мостового USDT на Base. Для обновляемых токенов V5 проверяет чистый результат в транзакции. Для всех девяти пулов используется существующий шаг V5 и обычный callback Uniswap V3.", "",
            "[Официальный интерфейс Virtus](https://liquidity.virtus-protocol.com/) · [Проверенная фабрика](https://repo.sourcify.dev/8453/0x0e5ab24bebda7e5bb3961f7e9b3532a83ae86b48) · [Архив кошелька](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45992658070)", ""]


def clone_classic_markets():
    root = Path("tests/data/first_run_dex_2026-11-26")
    lines = ["### Virtus AMM и Hydrometer на Base", "",
             "Обе фабрики создают минимальные клоны с неизменной реализацией. При вводе профиля сверяются код фабрики и реализации, адрес CREATE2 из обоих токенов и признака stable, прямой и обратный реестры, физические остатки, пауза и ставка getFee(pool,stable). Для штатного Router дополнительно сверяются код, defaultFactory, одобрение реестром и poolFor. Вход поддерживается при подтверждённом точном исходящем переводе. Конечный минимум штатный роутер проверяет по изменению баланса получателя. Для остальных токенов остаётся существующий V5, шаг 3.", "",
             "| Профиль | Stable / volatile | Рынки с выплатой | Выплаты Router и V5 | Газ Router минус V5 до публикации |",
             "|---|---:|---:|---:|---:|"]
    for name in ["virtus_amm","hydrometer"]:
        b=json.loads(gzip.decompress((root/(name+"_base_wallets.json.gz")).read_bytes()))
        stable=sum(m["stable"] for m in b["markets"])
        payments={}
        for r in b["cases"]:
            payments.setdefault((r["pool"],r["input"],r["spent"]),{})[r["wire"]]=r
        gas=[]
        for p in payments.values():
            assert p["router"]["received"]==p["v5"]["received"]
            gas.append(p["router"]["gas"]-p["v5"]["gas"])
        lines.append(f"| {name.replace('_','-')}-base | {stable} / {len(b['markets'])-stable} | {len({k[0] for k in payments})} | {len(b['cases'])} | {min(gas)}…{max(gas)} |")
    lines.extend(["", "На блоке 62262756 в пуле Hydrometer WETH/USDC резерв одной стороны составлял одну минимальную единицу. При размерах покупки, полученных из 10¹² и 10¹⁴ wei, оба способа исполнения вернули отказ. Ставки за обмен выражены в долях от 10 000. Стабильная кривая в ветви с нулевым шагом Ньютона повторно нормализует один пробный баланс с исходными decimals0 и decimals1. Котировка и повторная нога используют реальное округление пары.", "",
                  "[Фабрика Virtus AMM](https://repo.sourcify.dev/8453/0x7f03ae4452192b0e280fb0d4f9c225dda88c7623) · [Router Hydrometer](https://repo.sourcify.dev/8453/0xed6d68b9048a1842e31ab41ebfd1262e501fbea5) · [Архив обеих площадок](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45995605106)", ""])
    return lines
