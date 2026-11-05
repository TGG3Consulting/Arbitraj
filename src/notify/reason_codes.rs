//! Файл: notify/reason_codes.rs
//! Что делает: словарь человеческих формулировок для известных кодов ошибок бирж (решение 164). Смысл каждого кода —
//! только из комментариев адаптеров и ссылок на официальную документацию в них (venues/<площадка>/errors.rs,
//! venues/mexc/deposit_hold.rs); новых смыслов здесь не придумывается. Кода нет в словаре — алерт показывает
//! «Ответ биржи: код и текст как есть».
//! Кто вызывает: notify/reason.rs.
//! От чего зависит: ничего.
//! Настройки: нет.

/// Человеческая формулировка кода. Принимает: id площадки, код (число строкой или метка Gate). Возвращает: текст или None.
pub fn lookup(venue: &str, code: &str) -> Option<&'static str> {
    match venue {
        "bingx" => bingx(code),
        "mexc" => mexc(code),
        "gate" => gate(code),
        "bybit" => bybit(code),
        "kucoin" => kucoin(code),
        "htx" => htx(code),
        _ => None,
    }
}

/// BingX — venues/bingx/errors.rs (Error Code Reference).
fn bingx(code: &str) -> Option<&'static str> {
    Some(match code {
        "100001" | "100412" | "100413" => "ключ API или подпись не приняты",
        "100004" => "у ключа API нет нужного права",
        "100419" => "IP сервера не в белом списке ключа",
        "100410" => "слишком частые запросы — сработал лимит BingX",
        "100202" => "не хватает средств",
        "100500" => "система BingX занята — запрос не выполнен",
        _ => return None,
    })
}

/// MEXC — venues/mexc/errors.rs (Error Code), 16021 — venues/mexc/deposit_hold.rs.
fn mexc(code: &str) -> Option<&'static str> {
    Some(match code {
        "10072" | "700001" | "700002" | "602" => "ключ API или подпись не приняты",
        "700003" | "10073" => "время запроса не совпало с часами MEXC — проверьте часы сервера",
        "700007" | "401" | "30020" | "70011" => "у ключа API нет нужного права",
        "30035" => "сработал лимит ордеров аккаунта — нужна заявка в поддержку MEXC",
        "60005" => "MEXC считает аккаунт необычным (account is abnormal)",
        "700006" => "IP сервера не в белом списке ключа",
        "429" => "слишком частые запросы — сработал лимит MEXC",
        "10101" => "не хватает средств",
        "30004" | "30005" => "продаётся больше, чем есть на балансе",
        "11444" => "функция MEXC временно недоступна",
        "500" | "503" | "504" | "20002" => "сервер MEXC занят — итог запроса нужно проверить",
        "-2011" => "ордер не найден",
        "16021" => "MEXC ещё не до конца зачислил депозит — вывод временно запрещён",
        _ => return None,
    })
}

/// Gate — метки из venues/gate/errors.rs (API v4, «Error label list»).
fn gate(label: &str) -> Option<&'static str> {
    Some(match label {
        "INVALID_CREDENTIALS" | "INVALID_KEY" | "INVALID_SIGNATURE" | "MISSING_REQUIRED_HEADER" => {
            "ключ API или подпись не приняты"
        }
        "REQUEST_EXPIRED" => "время запроса далеко от часов Gate — проверьте часы сервера",
        "IP_FORBIDDEN" => "IP сервера не в белом списке ключа",
        "READ_ONLY" => "ключ API только на чтение",
        "FORBIDDEN" => "доступ запрещён",
        "ACCOUNT_LOCKED" => "аккаунт Gate заблокирован",
        "API_WITHDRAW_DISABLED" => "вывод через API выключен",
        "TOO_MANY_REQUESTS" => "слишком частые запросы — сработал лимит Gate",
        "TOO_FAST" => "слишком частый вывод — Gate ограничивает частоту выводов, нужно подождать",
        "BALANCE_NOT_ENOUGH" | "QUANTITY_NOT_ENOUGH" => "не хватает средств",
        "ADDRESS_NOT_USED" => {
            "Gate не разрешает вывод на этот адрес — адрес не подтверждён в адресной книге Gate"
        }
        "WITHDRAWAL_OVER_LIMIT" => "сумма вывода выше лимита Gate",
        "DUPLICATE_REQUEST" | "REPEATED_CREATION" => "повторный запрос",
        "INVALID_PRECISION" => "неверная точность числа",
        "INVALID_CURRENCY" => "Gate не знает такую монету",
        "INVALID_CURRENCY_PAIR" => "Gate не знает такую пару",
        "ORDER_NOT_FOUND" => "ордер не найден",
        "ORDER_CLOSED" => "ордер уже закрыт",
        "ORDER_CANCELLED" => "ордер уже отменён",
        "AMOUNT_TOO_LITTLE" => "сумма меньше минимума Gate",
        "AMOUNT_TOO_MUCH" => "сумма больше максимума Gate",
        "INTERNAL" | "SERVER_ERROR" | "INTERNAL_SERVER_ERROR" => "ошибка на стороне Gate",
        "TOO_BUSY" => "Gate перегружен",
        _ => return None,
    })
}

/// Bybit — venues/bybit/errors.rs (Error Codes V5).
fn bybit(code: &str) -> Option<&'static str> {
    Some(match code {
        "10003" | "10004" | "10007" | "-2015" | "33004" => "ключ API или подпись не приняты",
        "10002" | "-1" => "время запроса вне окна Bybit — проверьте часы сервера",
        "10005" => "у ключа API нет нужного права",
        "10008" | "10009" | "10024" | "10027" | "10028" => "Bybit ограничил аккаунт",
        "10010" => "IP сервера не в белом списке ключа",
        "10006" | "429" | "170005" | "170222" => "слишком частые запросы — сработал лимит Bybit",
        "110004" | "110007" | "110012" | "170131" | "131212" | "131228" | "131085" => {
            "не хватает средств"
        }
        "131083" | "131093" => "адреса нет в адресной книге (белом списке) Bybit",
        "10000" | "10016" | "170001" | "170007" | "170146" => {
            "сервер Bybit не ответил вовремя — итог нужно проверить"
        }
        "110001" | "170213" => "ордер не найден",
        _ => return None,
    })
}

/// KuCoin — venues/kucoin/errors.rs (Classic Spot / Classic HTTP).
fn kucoin(code: &str) -> Option<&'static str> {
    Some(match code {
        "400001" | "400003" | "400004" | "400005" => "ключ API, passphrase или подпись не приняты",
        "400002" => "время запроса не принято — проверьте часы сервера",
        "400006" => "IP сервера не в белом списке ключа",
        "400007" => "доступ запрещён",
        "411100" => "аккаунт KuCoin заморожен",
        "400700" => "KuCoin ограничил аккаунт по риску",
        "400500" => "недоступно в регионе",
        "200009" => "KuCoin требует KYC",
        "429000" => "слишком частые запросы — сработал лимит KuCoin",
        "200004" | "102421" => "не хватает средств",
        "115007" => "вывод только с основного (main) счёта KuCoin",
        "260210" => "вывод монеты по этой сети закрыт или аккаунт заморожен",
        "110003" => "превышен суточный предел вывода",
        "500000" => "ошибка на стороне KuCoin",
        "230005" => "система KuCoin занята",
        _ => return None,
    })
}

/// HTX — venues/htx/errors.rs (Spot API: Trading → Error Code, Wallet → Error Code).
fn htx(code: &str) -> Option<&'static str> {
    Some(match code {
        "login-required" | "api-signature-not-valid" | "1002" | "1003" => {
            "ключ API или подпись не приняты"
        }
        "order-accountbalance-error" => "не хватает средств",
        "order-value-min-error" => "сумма ордера меньше минимума HTX",
        "order-orderprice-precision-error" => "неверная точность цены",
        "order-orderamount-precision-error" => "неверная точность количества",
        "order-invalid-price" => "цена ордера слишком далеко от последней сделки",
        "invalid-client-order-id" => "повтор своего ID ордера",
        "base-symbol-trade-disabled" | "order-disabled" => "торговля парой на HTX закрыта",
        "base-system-error" | "500" => "система HTX занята — итог запроса нужно проверить",
        "2002" | "2003" => "HTX не принял параметр запроса",
        _ => return None,
    })
}
