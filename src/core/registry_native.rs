//! Файл: core/registry_native.rs
//! Что делает: решение 211, ревью К1 — нативная монета сети (BNB) в справочнике по адресу. Биржа отдаёт нативную BNB
//! без адреса; по решению 62 (native.rs) ей ставится адрес обёртки (WBNB), чтобы она совпала с WBNB на DEX. Если
//! другая биржа держит в сети BSC НАСТОЯЩИЙ BEP20-токен обёртки (тот же адрес, свой тикер, например «WBNB»), это другой
//! актив: перевод нативной BNB на адрес депозита токена (и наоборот) не зачислится. Поэтому у такой записи
//! присутствие-токен исключается (NativeVsToken), направлений «нативная ↔ токен» нет; показное имя монеты — тикер
//! нативной (dex.native_ticker): по нему DEX и ядро узнают нативную (exec/native.rs, core/native.rs, sell.rs).
//! Нативной на биржах нет — запись токена обёртки обычная монета по адресу.
//! Кто вызывает: core/registry_build.rs.
//! От чего зависит: native.rs (Native), registry.rs (Presence, Exclusion).
//! Настройки: через Native — dex.native_ticker, dex.native_wrapped.

use alloy_primitives::Address;

use super::native::Native;
use super::registry::{Exclusion, Presence};

/// Делит нативную и токен обёртки. Принимает: присутствия бирж записи (по одному на биржу), адрес записи, нативную
/// монету. Возвращает: Some(тикер нативной) — запись нативной (показное имя), иначе None. Ошибок нет.
pub fn split(
    list: &mut [Presence],
    contract: Option<Address>,
    native: Option<&Native>,
) -> Option<String> {
    let n = native?;
    if contract != Some(n.wrapped) || !list.iter().any(|p| p.ticker == n.ticker) {
        return None;
    }
    for p in list.iter_mut().filter(|p| p.ticker != n.ticker) {
        p.excluded = vec![Exclusion::NativeVsToken(format!(
            "{}: токен обёртки «{}», а на других биржах это нативная {}",
            p.venue.0, p.ticker, n.ticker
        ))];
        p.can_buy = false;
        p.can_sell = false;
        p.can_send = false;
        p.can_receive = false;
    }
    Some(n.ticker.clone())
}
