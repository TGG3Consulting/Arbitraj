//! Файл: notify/human.rs
//! Что делает: человеческий вид алертов (решение 164): числа для глаз (USDT и проценты — 2 знака, количество монеты —
//! без хвоста из 20 знаков), имена площадок (BingX, MEXC, DEX…), короткий адрес и сборщик сообщения `Msg`:
//! по единому шаблону решения 251 (разделы — notify/alert.rs: заголовок, «Сделка», карточка сделки, факты, «Цифры»,
//! «Причина», «Что делает бот», «Что нужно от вас», «Подробно») — сам сборщик в notify/human_msg.rs, здесь он
//! переэкспортирован.
//! Кто вызывает: ядро (тексты событий Telegram, записи «Ручного управления», хронология сделки), notify/dispatch.rs.
//! От чего зависит: rust_decimal, notify/reason.rs (человеческая причина).
//! Настройки: нет — это формат отображения, не торговые параметры (точность показа не влияет на расчёт).

use rust_decimal::Decimal;

pub use super::human_msg::{Msg, from_core, split_deal};

/// Значки уровня — первая строка сообщения. По первому символу dispatch.rs понимает, что заголовок уже есть.
pub const STOP: &str = "⛔";
pub const WARN: &str = "⚠️";
pub const OK: &str = "✅";
pub const LOSS: &str = "❌";
pub const INFO: &str = "ℹ️";
/// Решение 230: пауза площадки или кошелька от сверки (короткое сообщение risk.reconcile_paused).
pub const PAUSE: &str = "⏸";

/// Все значки уровня (для проверки «заголовок уже есть»).
pub const ICONS: [&str; 6] = [STOP, WARN, OK, LOSS, INFO, PAUSE];

/// Знаков после запятой у денег и процентов.
const MONEY_DP: u32 = 2;
/// Знаков у денег, если по модулю меньше 0.01 (чтобы не было «0.00»).
const SMALL_MONEY_DP: u32 = 4;
/// Значащих цифр у количества монеты.
const QTY_DIGITS: u32 = 4;
/// Сколько символов адреса показывать с каждого края.
const ADDR_EDGE: usize = 4;

/// Имя площадки для человека по id адаптера (venues/*/ctx.rs, ID). Неизвестная — id как есть.
/// Принимает: id площадки («bingx», «bsc_dex»…). Возвращает: «BingX», «DEX»…
pub fn venue(id: &str) -> String {
    match id {
        "bingx" => "BingX",
        "mexc" => "MEXC",
        "gate" => "Gate",
        "kucoin" => "KuCoin",
        "poloniex" => "Poloniex",
        "htx" => "HTX",
        "bybit" => "Bybit",
        "phemex" => "Phemex",
        "kraken" => "Kraken",
        "coinex" => "CoinEx",
        "bsc_dex" => "DEX",
        "kyberswap" => "KyberSwap",
        "xt" => "XT",
        // Решение 251: путь сделки — именами всех подключённых площадок (venues/*/ctx.rs, ID).
        "bitget" => "Bitget",
        "okx" => "OKX",
        "binance" => "Binance",
        other => other,
    }
    .to_string()
}

/// Деньги по модулю без знака: «44.98 USDT» (меньше 0.01 — 4 знака). Принимает: сумму.
pub fn usd(d: Decimal) -> String {
    format!("{} USDT", money(d.abs()))
}

/// Деньги со знаком: «+0.09 USDT», «−0.15 USDT». Принимает: сумму.
pub fn signed_usd(d: Decimal) -> String {
    let sign = if d.is_sign_negative() && !d.is_zero() {
        "−"
    } else {
        "+"
    };
    format!("{sign}{} USDT", money(d.abs()))
}

/// Число денег: 2 знака, по модулю меньше 0.01 (и не ноль) — 4 знака. Принимает: сумму. Возвращает: строку.
pub fn money(d: Decimal) -> String {
    let dp = if !d.is_zero() && d.abs() < Decimal::new(1, MONEY_DP) {
        SMALL_MONEY_DP
    } else {
        MONEY_DP
    };
    let s = format!("{:.*}", dp as usize, d.round_dp(dp));
    s.replace('-', "−")
}

/// Проценты: «1.23%». Принимает: проценты (уже в процентах, не в долях).
pub fn pct(d: Decimal) -> String {
    format!("{}%", money(d))
}

/// Решение 247, п. Е (живой случай PIG 09.11: «Убыток при продаже сейчас −2.03 %» — на деле прибыль): итог «продать
/// сейчас» по убытку Deal::loss_pct_if (плюс — убыток). Возвращает: (подпись, число): убыток > 0 — («Убыток при
/// продаже сейчас», «2.03%»); иначе — («Прибыль при продаже сейчас», «+2.03%»). Ошибок нет.
pub fn sale_now(loss: Decimal) -> (&'static str, String) {
    if loss > Decimal::ZERO {
        ("Убыток при продаже сейчас", pct(loss))
    } else {
        ("Прибыль при продаже сейчас", format!("+{}", pct(-loss)))
    }
}

/// Решение 247, п. Е: подпись и число «продать сейчас» для алерта (sale_now) с пометкой, если продажа по этой же
/// модели откатилась по minOut (текст отказа содержит «minOut»: число — из провалившейся модели). Принимает: убыток %
/// (None — нет котировки), текст отказа продажи. Возвращает: (подпись, значение). Ошибок нет.
pub fn sale_now_alert(loss: Option<Decimal>, error: Option<&str>) -> (&'static str, String) {
    let Some(l) = loss else {
        return (
            "Итог при продаже сейчас",
            "не известен (нет котировки)".into(),
        );
    };
    let (k, v) = sale_now(l);
    if min_out_failed(error) {
        return (k, format!("{v} (по модели — продажа по ней откатилась)"));
    }
    (k, v)
}

/// Откатилась ли продажа по minOut (текст отказа адаптера DEX: «… execution reverted: minOut …»). Ошибок нет.
pub fn min_out_failed(error: Option<&str>) -> bool {
    error.is_some_and(|e| e.contains("minOut"))
}

/// Решение 247, п. Е: подсказка после отката по minOut — что делает бот и когда нужна ручная продажа. None — не тот
/// отказ. Ошибок нет.
pub fn min_out_hint(error: Option<&str>) -> Option<&'static str> {
    min_out_failed(error).then_some(
        "реальный выход ниже модели (налог монеты выше модельного). Бот на повторе и по кнопке «Продать сейчас» \
         пере-меряет выход симуляцией и продаёт с minOut от наименьшего из замера и выхода при налоге, наблюдённом в \
         сети (решение 247). Продавать руками — если пере-замер тоже откатился или граница ниже порога убытка",
    )
}

/// Решение 247, п. Е: заголовок алерта по настоящей причине: отказ продажи — «продажа не прошла»; иначе по убытку.
/// Принимает: убыток % (None — нет котировки), был ли отказ продажи. Ошибок нет.
pub fn alert_head(loss: Option<Decimal>, failed: bool) -> &'static str {
    match (failed, loss) {
        (true, _) => "Нужно ваше решение: продажа не прошла",
        (false, Some(l)) if l > Decimal::ZERO => "Нужно ваше решение: монета застряла с убытком",
        (false, _) => "Нужно ваше решение: монета застряла",
    }
}

/// Решение 247, п. Е: убыток против порога словами по знаку (логика порога — у вызывающего). Принимает: убыток %
/// (Deal::loss_pct_if), порог %, в пределах ли порога. Возвращает: убыток > 0 — «убыток 2% не больше / выше порога
/// 5%» (как раньше); иначе — «прибыль +2% (порог убытка 5%)» / «прибыль +2% — порог убытка −5% не пройден».
pub fn loss_vs_max(loss: Decimal, max: Decimal, within: bool) -> String {
    match (loss > Decimal::ZERO, within) {
        (true, true) => format!("убыток {} не больше порога {}", pct(loss), pct(max)),
        (true, false) => format!("убыток {} выше порога {}", pct(loss), pct(max)),
        (false, true) => format!("прибыль +{} (порог убытка {})", pct(-loss), pct(max)),
        (false, false) => format!(
            "прибыль +{} — порог убытка {} не пройден",
            pct(-loss),
            pct(max)
        ),
    }
}

/// Количество монеты: 4 значащие цифры, целая часть — целиком, без хвостовых нулей («139708165163», «44.98»,
/// «0.004052»). Принимает: количество.
pub fn qty(d: Decimal) -> String {
    let a = d.abs();
    let int_digits = a.trunc().to_string().trim_start_matches('0').len() as u32;
    let dp = if int_digits >= QTY_DIGITS {
        0
    } else if int_digits > 0 {
        QTY_DIGITS - int_digits
    } else {
        // Меньше единицы: нули после запятой + значащие цифры.
        let mut zeros = 0;
        let mut x = a;
        while !x.is_zero() && x < Decimal::new(1, 1) && zeros < 28 {
            x *= Decimal::TEN;
            zeros += 1;
        }
        (zeros + QTY_DIGITS).min(28)
    };
    d.round_dp(dp).normalize().to_string().replace('-', "−")
}

/// Короткий адрес «0xc748…e8de». Короткие строки — как есть. Принимает: адрес.
pub fn addr(a: &str) -> String {
    let n = a.chars().count();
    if n <= 2 + ADDR_EDGE * 2 + 1 {
        return a.to_string();
    }
    let head: String = a.chars().take(2 + ADDR_EDGE).collect();
    let tail: String = a.chars().skip(n - ADDR_EDGE).collect();
    format!("{head}…{tail}")
}

/// «откуда → куда» именами площадок. Принимает: id площадок.
pub fn route(from: &str, to: &str) -> String {
    format!("{} → {}", venue(from), venue(to))
}

/// Есть ли у текста свой заголовок со значком уровня. Принимает: текст.
pub fn has_head(text: &str) -> bool {
    ICONS.iter().any(|i| text.starts_with(i))
}
