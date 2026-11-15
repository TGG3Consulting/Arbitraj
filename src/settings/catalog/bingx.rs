//! Файл: settings/catalog/bingx.rs
//! Что делает: настройки, нужные только бирже BingX, — решение 293: не соблюдать устаревшие minQty / maxQty справочника
//! пар (venues/bingx/symbols_qty.rs). Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «Площадки — BingX»).

use super::{V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки решения 293. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static BINGX: &[SettingDef] = &[
    d("Площадки", "venue.ignore_deprecated_qty", K::Bool, L::None, "да/нет", V, "Решение 293, только BingX: не соблюдать minQty и maxQty справочника пар. В официальной документации BingX (docs-v3 «Spot trading symbols») оба поля помечены «Deprecated; can be calculated using minQty=minNotional/price» — https://bingx-api.github.io/docs-v3/#/en/Spot/Market%20Data/Spot%20trading%20symbols. Пример: у SHOPINX-USDT 15.11 minQty 57 505 монет (≈500 USDT при цене 0,0087), а продажи по 8–11 тыс. SHOPINX (≈100 USDT) на BingX проходили; с minQty бот такую монету не проверяет и не торгует. Да — у всех пар BingX пределов количества нет: ордер, проверка ордера до покупки и подбор объёма смотрят только на minNotional / maxNotional (сумма ордера), шаги и статус пары. Если биржа всё же откажет ордеру — как любой отказ: покупка не повторяется; продажа после покупки — алерт «продажа не прошла» на «Ручное управление», монеты на бирже. Действует со следующего чтения справочника пар (venue.symbols_refresh_s). Нет или не задана — оба предела соблюдаются, как раньше (решение 66)").optional(),
];
