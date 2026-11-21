//! Файл: venues/bsc_dex/pools/wrapped.rs
//! Что делает: решение 270 — делит WETH() роутеров V2 на «обёртку нативной монеты» и «чужую обёртку». Нативной
//! (тикер BNB, 1:1) считается только dex.native_wrapped; WETH() роутера, не равный ей (факт цепи 14.11.2026: у роутера
//! fstswap WETH() = 0x0EFb5fD2402A0967b92551d6AF54de148504A115, имя «Wrapped BNB», это НЕ WBNB 0xbb4C…), — чужой
//! токен: ему тикер BNB не даётся, маршруты через него не берутся. dex.native_wrapped не задана — делить не с чем,
//! все WETH() — нативные (поведение до решения 270). У DEX с WETH() = dex.native_wrapped результат тот же, что был.
//! natives — какие адреса получают тикер нативной монеты (при dex.intermediate_dex_priced = да — только
//! dex.native_wrapped, решение 270 дополнение 2).
//! Кто вызывает: pools/mids.rs (тикер промежуточной монеты), connection_extra.rs («Проверить подключение»).
//! От чего зависит: alloy_primitives.
//! Настройки: dex.native_wrapped, dex.intermediate_dex_priced (передаются параметрами).

use alloy_primitives::Address;

/// WETH() роутера: название DEX и адрес из ответа роутера.
pub type RouterWeth = (String, Address);

/// Делит WETH() роутеров. Принимает: список (DEX, WETH()), dex.native_wrapped (None — не задана).
/// Возвращает: (нативные обёртки — адреса; чужие — (DEX, адрес)). Не задана — все нативные, чужих нет.
/// Чистая функция, ошибок нет.
pub fn split(list: Vec<RouterWeth>, native: Option<Address>) -> (Vec<Address>, Vec<RouterWeth>) {
    let Some(n) = native else {
        return (list.into_iter().map(|(_, a)| a).collect(), Vec::new());
    };
    let (own, foreign): (Vec<_>, Vec<_>) = list.into_iter().partition(|(_, a)| *a == n);
    (own.into_iter().map(|(_, a)| a).collect(), foreign)
}

/// Нативные обёртки для тикера промежуточной монеты (pools/mids.rs). Принимает: WETH() роутеров, dex.native_wrapped
/// (None — не задана), dex.intermediate_dex_priced. Возвращает: (адреса, которым дать тикер нативной монеты; чужие
/// WETH() — для текста отказа). Флаг нет: split и плюс dex.native_wrapped (решение 205: у части форков WETH() нет) —
/// как до решения 276, включая «не задана — все WETH() нативные». Флаг да (решение 270, дополнение 2): нативная —
/// только dex.native_wrapped (не задана — ни одной); остальные WETH() идут веткой решения 276. Чистая, ошибок нет.
pub fn natives(
    list: Vec<RouterWeth>,
    configured: Option<Address>,
    dex_priced: bool,
) -> (Vec<Address>, Vec<RouterWeth>) {
    let (mut own, foreign) = split(list, configured);
    if dex_priced {
        own = configured.into_iter().collect();
    } else if let Some(w) = configured
        && !own.contains(&w)
    {
        own.push(w);
    }
    (own, foreign)
}

/// Факт о чужой обёртке для журнала и экрана. Чистая, ошибок нет.
pub fn foreign_fact(title: &str, weth: Address) -> String {
    format!(
        "WETH() роутера {title} = {weth}, это не dex.native_wrapped — маршруты через эту обёртку не берутся"
    )
}

/// Итог «Проверить подключение» по обёртке (решение 270). Принимает: dex.native_wrapped (контракт по адресу есть) и
/// WETH() роутеров V2. Возвращает: Ok(описание) — хотя бы один роутер вернул dex.native_wrapped (чужие — фактом в
/// описании) или сверять не с чем (роутеров нет); Err(причина) — роутеры есть, и ни один её не вернул: адрес в
/// настройке, вероятно, вписан неверно (как до решения 270). Чистая функция.
pub fn verdict(addr: Address, list: Vec<RouterWeth>) -> Result<String, String> {
    let (own, foreign) = split(list, Some(addr));
    match (own.is_empty(), foreign.first()) {
        (true, Some((_, w))) => Err(format!(
            "WETH() роутера V2 = {w}, а dex.native_wrapped = {addr} — проверьте адрес"
        )),
        (true, None) => Ok(format!(
            "контракт {addr} есть (сверить с WETH() не с чем: нет включённых DEX вида V2 с dex.router)"
        )),
        (false, _) => {
            let tail: String = foreign
                .iter()
                .map(|(t, a)| format!("; {}", foreign_fact(t, *a)))
                .collect();
            Ok(format!(
                "контракт {addr} есть, совпадает с WETH() роутеров V2{tail}"
            ))
        }
    }
}
