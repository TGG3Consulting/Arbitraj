"""Сведения о фактическом реестре, ставках и выплатах из архивов общего справочника."""
import gzip
import json
from pathlib import Path

DATA = Path("tests/data/first_run_dex_2026-11-26")


def snapshot(name):
    return json.loads(gzip.decompress((DATA / name).read_bytes()))


def paragraphs(network):
    if network == "ethereum":
        mars = snapshot("marswap-ethereum.json.gz")
        rows = [r for r in mars["markets"] if "settlements" in r]
        samples = []
        for row in rows:
            order = next(c for c in row["settlements"]["cases"] if c["mode"] == "percentage")
            buy, sell = order["buy"], order["sell"]
            samples.append(f"| `{row['pool']}` | {buy['input']} → {buy['net']} → {sell['net']} | {sell['nominal']} |")
        _, lp, platform, flat = mars["terms"]
        return [
            "#### MarSwap на Ethereum", "",
            f"У фабрики `{mars['factory']}` зарегистрировано {mars['markets_count']} CREATE2-пар. "
            f"На блоке {mars['height']} у двух рынков ненулевые резервы, остальные три пусты. "
            f"Ставка обычного обмена равна ({lp} + {platform}) / 10 000 из `lpInfo()`. "
            f"Оплата {flat} wei ETH на один шаг отключает процентную ставку у проверенного Router02. "
            "На момент снимка это дороже совокупных 115 301 178 451 469 wei WETH в обеих активных парах. "
            "Первый запуск предлагает процентный Router02 и V5. Измеренный режим с фиксированной платой требует отдельного учёта стоимости ETH в заказе.",
            "", "| Контракт пары | Вход WETH, фактическая покупка токена, обратная выплата WETH (атомы) | Расчётная обратная выплата по резервам до налога (wei) |",
            "|---|---:|---:|", *samples, "",
            "Проверены два размера, оба направления и три режима в каждой из двух пар. Процентный роутер и V5 дали одинаковую чистую выплату. "
            "Налог токена зависит от направления. Сигнал использует фактическую выплату и полное списание выбранного кошелька. "
            "[Запросы, отказы и остатки](../tests/data/first_run_dex_2026-11-26/marswap-ethereum.json.gz).",
            "", "У `dex-on-crypto-ethereum` публичный каталог вернул пустую страницу рынка. Фабрика, адрес исполняемого контракта и обе выплаты для этой сети остаются открытыми.",
            ""
        ]
    omni = snapshot("omni-v2-base.json.gz")
    mars = snapshot("marswap-base.json.gz")
    example = next(e for e in omni["examples"] if "wallet" in e)
    wallet = example["wallet"]
    deals = []
    for case in wallet["cases"]:
        direct, executor = case["native"], case["v5"]
        totals = [str(int(c[leg]["results"][-3]["gasUsed"], 16)) for c in (direct, executor) for leg in ("buy","sell")]
        deals.append(f"| {direct['buy']['input']} → {direct['buy']['output']} → {direct['sell']['output']} | "
                     f"{totals[0]} / {totals[1]} | {totals[2]} / {totals[3]} |")
    return [
        "#### Omni Exchange V2 и MarSwap на Base", "",
        f"У OmniFactory `{omni['factory']['address']}` подтверждены {len(omni['markets'])} CREATE2-пар. "
        f"Фиксированная комиссия равна 25/10 000; проверенный хеш создания `{omni['init_hash']}`. "
        f"В паре `{example['pool']}` кошелёк `{wallet['owner']}` выполнил защищённые обмены через "
        f"Universal Router `{omni['router']['address']}`, его собственный Permit2 `{wallet['permit2']}` и V5. "
        "При первом запуске доступны V5 и стандартный путь через Omni Universal Router с проверкой встроенных адресов. Строгий минимум выше выплаты на один атом и истёкший срок отклонены в каждом направлении. "
        "У других зарегистрированных пар результат зависит от токенов, суммы и состояния своей пары.",
        "", "| Вход WETH → токен → WETH (атомы) | Газ команды Universal Router, покупка/продажа | Газ команды V5, покупка/продажа |",
        "|---:|---:|---:|", *deals, "",
        "Газ разрешений оплачивается отдельно при недостаточном allowance. "
        "[Исходник роутера с привязкой Permit2 и фабрики](https://sourcify.dev/server/v2/contract/8453/0x423fae8ee58833f283e8845479bd705514110ec3?fields=all) · "
        "[Каталог и фактические выплаты](../tests/data/first_run_dex_2026-11-26/omni-v2-base.json.gz).",
        "", f"У фабрики MarSwap `{mars['factory']}` на блоке {mars['height']} метод `allPairsLength()` вернул {mars['markets_count']}. "
        f"Её `INIT_CODE_PAIR_HASH()` совпадает с Ethereum: `{mars['init_hash']}`. Адреса фабрик этих сетей различаются. "
        "[Каноническая запись](../tests/data/first_run_dex_2026-11-26/marswap-base.json.gz).",
        "", "По `dex-on-crypto-base` и `dejungle` страницы пулов GeckoTerminal вернули пустые списки. "
        "В опубликованных материалах проекта DeJungle указатель на фабрику отсутствует. "
        "Для обеих площадок ещё требуются исходный адрес, его принадлежность протоколу на Base и проверяемый рынок.",
        "", "#### Deli Swap на Base", "",
        "События Initialize у Uniswap V4 PoolManager доказывают два поколения. Семь сохранённых PoolId используют "
        "DeliHookConstantProduct `0x95afbc0fccf974b41380f24e562f15b6dd90fac8`. Состояние каждого рынка хранится в getReserves(poolId) самого хука. "
        "У четырёх из них остаток wBLT на блоке снимка составлял от 5 до 1081 атома. "
        "Четыре отдельных рынка (wBLT/USDC на двух шагах цены, wBLT/cbBTC, wBLT/BMX) используют DeliHook "
        "`0x570a48f96035c2874de1c0f13c5075a05683b0cc` с концентрированной ликвидностью и управляемой ставкой. "
        "Для CL обычный кошелёк на двух объёмах в обоих направлениях успешно обменял wBLT/USDC (шаг 40), wBLT/cbBTC и wBLT/BMX; на отдельном wBLT/USDC (шаг 60) обе исходные котировки из wBLT отклонились. При отдельном старте с 2 и 100 000 атомов USDC оба кошельковых пути выплатили 1 009 638 069 109 и 99 488 139 484 180 048 атомов wBLT. При входе 1 атома USDC Quoter вернул 340282366920938463463374607431766854381, а отправка с минимумом в 1 атом wBLT на обоих путях отклонилась с нулевым движением балансов. [Котировки и остатки CL](../tests/data/first_run_dex_2026-11-26/deli_base_cl_wallets.json.gz). Каждая модель вызывает внешние контракты учёта комиссии и вознаграждений. "
        "В семи пулах constant-product получены 20 полных циклов Router/V5 и 46 исполненных ног из 56; на четырёх пылевых "
        "рынках меньший размер округляется до нуля хотя бы в одну сторону. Живой снимок связывает каждый ключ с кодом хука, "
        "кодом feeProcessor и обеих gauges. После внешних вызовов программа разрешает отдельный путь с подтверждённым минимумом. "
        "[Выплаты кошельку и газ по каждому размеру](../tests/data/first_run_dex_2026-11-26/deli_base_wallets.json.gz) · "
        "[Initialize постоянного произведения](../tests/data/first_run_dex_2026-11-26/deli_base_initializations.json) · "
        "[Initialize концентрированной ликвидности](../tests/data/first_run_dex_2026-11-26/deli_base_cl_initializations.json) · "
        "[Исходники проекта](https://github.com/useboardwalk/deli-swap-contracts).", ""
    ]
