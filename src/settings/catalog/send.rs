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
    d("Отправка", "send.max_wait_blocks", K::Integer, L::Positive, "блоков", V, "Сколько блоков ждать квитанции, прежде чем отменить (0 BNB себе с тем же nonce); снова не занят — следующая отмена дороже").dex_venue(),
    d("Отправка", "send.replaced_confirm_blocks", K::Integer, L::Positive, "блоков", V, "Nonce занят, а своей квитанции нет: сколько ещё блоков искать её (квитанция и eth_getTransactionByHash), прежде чем «вытеснена»; у бандла — запас блоков после maxBlockNumber при позднем выяснении (решение 233)").dex_venue(),
    d("Отправка", "send.cancel_max_gas_gwei", K::Decimal, L::Positive, "gwei", V, "Потолок цены газа отмены; выше — отмен больше нет, очередь кошелька останавливается до ручной проверки").dex_venue(),
    d("Отправка", "send.cancel_max_attempts", K::Integer, L::Positive, "раз", V, "Сколько отмен одной транзакции; больше — очередь кошелька останавливается до ручной проверки").dex_venue(),
    d("Отправка", "send.queue_max_wait_s", K::Integer, L::Positive, "секунды", V, "Сколько очередь кошелька ждёт итога одной транзакции (узел недоступен, нет BNB на отмену); дольше — остановка до ручной проверки").dex_venue(),
    d("Отправка", "send.bundle_max_blocks", K::Integer, L::Positive, "блоков", V, "Бандл: maxBlockNumber = текущий блок + это число").dex_venue(),
    d("Отправка", "send.cancel_gas_bump_pct", K::Decimal, L::Positive, "%", V, "Не попала за send.max_wait_blocks — отмена: 0 BNB себе с тем же nonce, цена газа выше прошлой на столько %").dex_venue(),
    d("Кошелёк", "send.deposit_confirmations", K::Integer, L::NonNegative, "блоков", V, "Зачисление в кошелёк: «Зачислен» только когда после блока перевода прошло столько блоков и перевод на кошелёк ненулевой").dex_venue(),
    d("Каналы отправки", "channel.enabled", K::Bool, L::None, "да/нет", CH, "Канал включён (несколько закрытых — параллельно, без чаевых; открытый — только один)"),
    d("Каналы отправки", "channel.url", K::Url, L::None, "URL", CH, "Адрес канала из его официальной документации; ключ в адресе — как {key}"),
    d("Каналы отправки", "channel.method", K::Choice, L::None, "способ", CH, "Способ отправки: одиночная / бандл (решение 43)").choices(crate::core::types::SendMethod::CHOICES),
    d("Каналы отправки", "channel.min_gas_price_gwei", K::Decimal, L::Positive, "gwei", CH, "Минимальная цена газа канала; ниже — в канал не отправляется (решение 44)"),
    d("Каналы отправки", "channel.tip_enabled", K::Bool, L::None, "да/нет", CH, "Чаевые билдеру включены (только при отправке в один канал)"),
    d("Каналы отправки", "channel.tip_bnb", K::Decimal, L::Positive, "BNB", CH, "Сумма чаевых билдеру"),
    d("Каналы отправки", "channel.tip_address", K::Address, L::None, "адрес", CH, "Куда переводить чаевые — адрес из официальной документации канала"),
];
