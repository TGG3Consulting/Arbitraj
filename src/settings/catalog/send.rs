//! Файл: settings/catalog/send.rs
//! Что делает: настройки отправки транзакций DEX — общие (газ, ожидание) и по каждому каналу
//! (вкл/выкл, адрес, способ, минимальная цена газа, чаевые). Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, решения 33, 41–45).
//! Решение 283: у настроек области «площадка» здесь — .dex_venue() (значение читается только у площадки DEX; интерфейс
//! не показывает их на экране биржи). Чтение и запись это не меняет.
//!
//! Область «канал» (ch): id — канал из списка адаптера DEX (contract::send_channels).
//! Ключ канала — секрет channel.<id>.key; если ключ по документации канала стоит в адресе —
//! в channel.url пишется {key}.

use super::{CH, V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки отправки. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static SEND: &[SettingDef] = &[
    d("Отправка", "send.gas_limit_margin_pct", K::Decimal, L::NonNegative, "%", V, "Запас к оценке газа eth_estimateGas").dex_venue(),
    d("Отправка", "send.gas_price_markup_pct", K::Decimal, L::NonNegative, "%", V, "Надбавка к цене газа eth_gasPrice (не ниже минимума канала)").dex_venue(),
    d("Отправка", "send.max_fee_margin_pct", K::Decimal, L::NonNegative, "%", V, "Решение 435 (433-д): только при dex.gas_mode = eip1559_l1 (Arbitrum) — запас потолка цены газа транзакции типа 2: maxFeePerGas = baseFee блока головы × (1 + запас/100) + чаевые send.priority_fee_gwei (EIP-1559: транзакция заплатит min(maxFeePerGas, baseFee + чаевые); запас — чтобы рост baseFee до включения в блок не сделал транзакцию невключаемой). Кошелёк обязан иметь газ × maxFeePerGas + сумму (предпроверка exec/gas_fund.rs). При legacy (BSC) не читается. Не задана при eip1559_l1 — отправки площадки нет (NotConfigured), «Проверить подключение», часть «Газ сети» — причина").optional().dex_venue(),
    d("Отправка", "send.priority_fee_gwei", K::Decimal, L::NonNegative, "gwei", V, "Решение 435 (433-д): только при dex.gas_mode = eip1559_l1 (Arbitrum) — чаевые maxPriorityFeePerGas транзакции типа 2 (ставка очереди секвенсора, docs.arbitrum.io «Gas and fees»); газ в плане сделки = газ × (baseFee головы + чаевые). Сколько ставить, документация не говорит — вопрос Тиграну (решение 433, вопрос 1). При legacy (BSC) не читается. Не задана при eip1559_l1 — котировок и отправки площадки нет (NotConfigured), «Проверить подключение», часть «Газ сети» — причина").optional().dex_venue(),
    d("Отправка", "send.max_wait_blocks", K::Integer, L::Positive, "блоков", V, "Сколько блоков ждать квитанции, прежде чем отменить (0 BNB себе с тем же nonce); снова не занят — следующая отмена дороже").dex_venue(),
    d("Отправка", "send.replaced_confirm_blocks", K::Integer, L::Positive, "блоков", V, "Nonce занят, а своей квитанции нет: сколько ещё блоков искать её (квитанция и eth_getTransactionByHash), прежде чем «вытеснена»; у бандла — запас блоков после maxBlockNumber при позднем выяснении (решение 233)").dex_venue(),
    d("Отправка", "send.cancel_max_gas_gwei", K::Decimal, L::Positive, "gwei", V, "Потолок цены газа отмены; выше — отмен больше нет, очередь кошелька останавливается до ручной проверки").dex_venue(),
    d("Отправка", "send.cancel_max_attempts", K::Integer, L::Positive, "раз", V, "Сколько отмен одной транзакции; больше — очередь кошелька останавливается до ручной проверки").dex_venue(),
    d("Отправка", "send.queue_max_wait_s", K::Integer, L::Positive, "секунды", V, "Сколько очередь кошелька ждёт итога одной транзакции (узел недоступен, нет BNB на отмену); дольше — остановка до ручной проверки").dex_venue(),
    d("Отправка", "send.bundle_max_blocks", K::Integer, L::Positive, "блоков", V, "Бандл: maxBlockNumber = текущий блок + это число").dex_venue(),
    d("Отправка", "send.bundle_skip_single", K::Bool, L::None, "да/нет", V, "Решение 299: бандл (маршрут из нескольких транзакций — доли через разные роутеры, цикл DEX ↔ DEX) отправляется только в бандловые каналы (channel.method = bundle), одиночные каналы из этой отправки исключаются (видно в журнале и в событии bundle.sent); нужен хотя бы один готовый бандловый канал. Атомарность не меняется: eth_sendBundle, всё-или-ничего. Одиночные сделки (одна транзакция) идут во все готовые каналы, как раньше. Не задано/нет — включённый одиночный канал делает бандловый маршрут неисполнимым (сигнал, решение 121 B3)").optional().dex_venue(),
    d("Отправка", "send.cancel_gas_bump_pct", K::Decimal, L::Positive, "%", V, "Не попала за send.max_wait_blocks — отмена: 0 BNB себе с тем же nonce, цена газа выше прошлой на столько %").dex_venue(),
    d("Кошелёк", "send.deposit_confirmations", K::Integer, L::NonNegative, "блоков", V, "Зачисление в кошелёк: «Зачислен» только когда после блока перевода прошло столько блоков и перевод на кошелёк ненулевой").dex_venue(),
    d("Каналы отправки", "channel.enabled", K::Bool, L::None, "да/нет", CH, "Канал включён (несколько закрытых — параллельно, без чаевых; открытый — только один)"),
    d("Каналы отправки", "channel.url", K::Url, L::None, "URL", CH, "Адрес канала из его официальной документации; ключ в адресе — как {key}"),
    d("Каналы отправки", "channel.method", K::Choice, L::None, "способ", CH, "Способ отправки: одиночная / бандл (решение 43)").choices(crate::core::types::SendMethod::CHOICES),
    d("Каналы отправки", "channel.min_gas_price_gwei", K::Decimal, L::Positive, "gwei", CH, "Минимальная цена газа канала; ниже — в канал не отправляется (решение 44)"),
    d("Каналы отправки", "channel.tip_enabled", K::Bool, L::None, "да/нет", CH, "Чаевые билдеру включены (только при отправке в один канал)"),
    d("Каналы отправки", "channel.tip_bnb", K::Decimal, L::Positive, "BNB", CH, "Сумма чаевых билдеру"),
    d("Каналы отправки", "channel.tip_address", K::Address, L::None, "адрес", CH, "Куда переводить чаевые — адрес из официальной документации канала"),
    d("Каналы отправки", "channel.chain", K::Text, L::None, "id сети", CH, "Решение 435 (433-е): id сети канала (как dex.chain площадки DEX, например arbitrum у канала «Секвенсор Arbitrum»). Задана — канал отправляет транзакции только площадки DEX с той же dex.chain; не задана — канал сети по умолчанию (одна площадка DEX — всех её каналов, как раньше; несколько — площадки с dex.chain = dex.default_chain). Каналам BSC не задавать (живые настройки BSC не меняются); каналу sequencer — id сети площадки Arbitrum (arb_dex). Необязательна").optional(),
];
