//! Файл: venues/bsc_dex/pools/wrapped.rs
//! Что делает: решение 270 — делит WETH() роутеров V2 на «обёртку нативной монеты» и «чужую обёртку». Нативной
//! (тикер BNB, 1:1) считается только dex.native_wrapped; WETH() роутера, не равный ей (факт цепи 14.11.2026: у роутера
//! fstswap WETH() = 0x0EFb5fD2402A0967b92551d6AF54de148504A115, имя «Wrapped BNB», это НЕ WBNB 0xbb4C…), — чужой
//! токен: ему тикер BNB не даётся, маршруты через него не берутся. dex.native_wrapped не задана — делить не с чем,
//! все WETH() — нативные (поведение до решения 270). У DEX с WETH() = dex.native_wrapped результат тот же, что был.
//! Кто вызывает: pools/mids.rs (тикер промежуточной монеты), connection_extra.rs («Проверить подключение»).
//! От чего зависит: alloy_primitives.
//! Настройки: dex.native_wrapped (передаётся параметром).

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

/// Факт о чужой обёртке для журнала и экрана. Чистая, ошибок нет.
pub fn foreign_fact(title: &str, weth: Address) -> String {
    format!(
        "WETH() роутера {title} = {weth}, это не dex.native_wrapped — маршруты через эту обёртку не берутся"
    )
}
