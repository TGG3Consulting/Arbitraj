//! Файл: settings/catalog/send.rs
//! Что делает: настройки отправки транзакций DEX — общие (газ, ожидание) и по каждому каналу
//! (вкл/выкл, адрес, способ, минимальная цена газа, чаевые). Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, решения 33, 41–45).
//!
//! Область «канал» (ch): id — канал из списка адаптера DEX (contract::send_channels).
//! Ключ канала — секрет channel.<id>.key; если ключ по документации канала стоит в адресе —
//! в channel.url пишется {key}.

use super::{CH, V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки отправки. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static SEND: &[SettingDef] = &[
    d("Отправка", "send.gas_limit_margin_pct", K::Decimal, L::NonNegative, "%", V, "Запас к оценке газа eth_estimateGas"),
    d("Отправка", "send.gas_price_markup_pct", K::Decimal, L::NonNegative, "%", V, "Надбавка к цене газа eth_gasPrice (не ниже минимума канала)"),
    d("Отправка", "send.max_wait_blocks", K::Integer, L::Positive, "блоков", V, "Сколько блоков ждать попадания транзакции; дольше — не попала, без повторов"),
    d("Отправка", "send.bundle_max_blocks", K::Integer, L::Positive, "блоков", V, "Бандл: maxBlockNumber = текущий блок + это число"),
    d("Каналы отправки", "channel.enabled", K::Bool, L::None, "да/нет", CH, "Канал включён (несколько закрытых — параллельно, без чаевых; открытый — только один)"),
    d("Каналы отправки", "channel.url", K::Url, L::None, "URL", CH, "Адрес канала из его официальной документации; ключ в адресе — как {key}"),
    d("Каналы отправки", "channel.method", K::Choice, L::None, "способ", CH, "Способ отправки: одиночная / бандл (решение 43)").choices(crate::core::types::SendMethod::CHOICES),
    d("Каналы отправки", "channel.min_gas_price_gwei", K::Decimal, L::Positive, "gwei", CH, "Минимальная цена газа канала; ниже — в канал не отправляется (решение 44)"),
    d("Каналы отправки", "channel.tip_enabled", K::Bool, L::None, "да/нет", CH, "Чаевые билдеру включены (только при отправке в один канал)"),
    d("Каналы отправки", "channel.tip_bnb", K::Decimal, L::Positive, "BNB", CH, "Сумма чаевых билдеру"),
    d("Каналы отправки", "channel.tip_address", K::Address, L::None, "адрес", CH, "Куда переводить чаевые — адрес из официальной документации канала"),
];
