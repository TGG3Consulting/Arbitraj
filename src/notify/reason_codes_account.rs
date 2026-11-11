//! Файл: notify/reason_codes_account.rs
//! Что делает: решение 253, дополнение t82 (Тигран 10.11) — человеческие причины отказов вывода «по аккаунту» и «по
//! адресу» (коды AccountRestricted и InvalidAddress адаптеров). Смысл каждого кода — только из комментария у кода в
//! venues/<площадка>/errors.rs (там — ссылка на официальную таблицу ошибок биржи); новых смыслов здесь нет. Текст идёт в
//! алерт venue.account_problem «Проблема в аккаунте <биржа>: <причина> — исправьте» (core/withdraw_block.rs) и в алерт
//! venue.withdraw_limit «Лимит вывода <биржа> исчерпан» (лимиты, дополнение t85 — core/withdraw_limit_pause.rs).
//! Отдельный файл — чтобы notify/reason_codes.rs не рос.
//! Кто вызывает: notify/reason_codes.rs (lookup — сначала здесь).
//! От чего зависит: ничего.
//! Настройки: нет.

/// Человеческая причина отказа вывода по аккаунту или адресу. Принимает: id площадки, код. Возвращает: текст или None.
pub fn lookup(venue: &str, code: &str) -> Option<&'static str> {
    match venue {
        "binance" => binance(code),
        "mexc" => mexc(code),
        "bingx" => bingx(code),
        "kucoin" => kucoin(code),
        "bitget" => bitget(code),
        "bybit" => bybit(code),
        "okx" => okx(code),
        "gate" => gate(code),
        "xt" => xt(code),
        "poloniex" => poloniex(code),
        _ => None,
    }
}

/// Общие формулировки (повторяются у разных бирж).
const DAY_LIMIT: &str = "исчерпан суточный лимит вывода аккаунта";
const KYC: &str = "аккаунт не прошёл нужную верификацию личности (KYC)";
const RISK: &str = "вывод из аккаунта ограничен риск-контролем биржи";
const SUB: &str = "вывод с субаккаунта не разрешён — нужен основной аккаунт";

/// Binance — venues/binance/errors.rs (Wallet → Error Code).
fn binance(code: &str) -> Option<&'static str> {
    Some(match code {
        "-4004" => "ключ не вошёл в аккаунт Binance (не авторизован)",
        "-4006" => "вывод разрешён только с основного аккаунта Binance",
        "-4008" => "тег (memo) адреса получателя не прошёл проверку Binance",
        "-4013" => "в аккаунте Binance не включена 2FA",
        "-4014" => "вывод запрещён в первые 2 минуты после входа в аккаунт Binance",
        "-4015" => "вывод из аккаунта Binance ограничен",
        "-4016" => "вывод запрещён 24 часа после смены пароля аккаунта Binance",
        "-4017" => "вывод запрещён 24 часа после отключения 2FA в аккаунте Binance",
        "-4023" => "исчерпан суточный лимит вывода аккаунта Binance (24 часа)",
        "-4039" => "Binance не нашёл пользователя аккаунта",
        "-4046" => "в аккаунте Binance не подтверждено соглашение",
        _ => return None,
    })
}

/// MEXC — venues/mexc/errors.rs (Error Code).
fn mexc(code: &str) -> Option<&'static str> {
    Some(match code {
        "10212" => {
            "адреса получателя нет в списке разрешённых адресов вывода MEXC (или он недействителен)"
        }
        "157005" => {
            "вывод заблокирован защитой MEXC после смены пароля, 2FA или адресов — ждать конца срока"
        }
        "10265" | "10098" => RISK,
        "730100" => "MEXC считает состояние аккаунта необычным",
        _ => return None,
    })
}

/// BingX — venues/bingx/errors.rs (error-codes.md).
fn bingx(code: &str) -> Option<&'static str> {
    Some(match code {
        "100403" => SUB,
        "100441" => "аккаунт BingX в необычном состоянии или нужна расширенная верификация (KYC)",
        _ => return None,
    })
}

/// KuCoin — venues/kucoin/errors.rs (Classic Spot).
fn kucoin(code: &str) -> Option<&'static str> {
    Some(match code {
        "110003" => DAY_LIMIT,
        _ => return None,
    })
}

/// Bitget — venues/bitget/errors.rs (11000 — таблица, 40938 — живой ответ).
fn bitget(code: &str) -> Option<&'static str> {
    Some(match code {
        "11000" | "40938" => {
            "вывод разрешён только на адреса из адресной книги аккаунта — адреса получателя там нет"
        }
        _ => return None,
    })
}

/// Bybit — venues/bybit/errors.rs (Error Codes → Asset).
fn bybit(code: &str) -> Option<&'static str> {
    Some(match code {
        "131065" | "131004" => KYC,
        // Дополнение t85: коды прав у вывода (venues/bybit/errors.rs).
        "131090" => "вывод для пользователя Bybit запрещён (User withdraw has been banned)",
        "131091" => "вход в аккаунт Bybit заблокирован — вывод не разрешён",
        "131092" => "Bybit считает состояние пользователя необычным (User status is abnormal)",
        "131084" => "аккаунт Bybit переходит на единый (UTA) — вывод временно недоступен",
        "131086" | "131087" => RISK,
        "131088" => "исчерпан лимит вывода уровня верификации аккаунта",
        "131089" => "вывод запрещён 24 часа после изменения настроек безопасности аккаунта",
        "131094" => "пользователь не в белом списке Bybit",
        "131095" => DAY_LIMIT,
        "131066" => "Bybit временно не разрешает вывод на этот адрес",
        "131098" => "вывод на новый адрес Bybit пока не разрешён",
        _ => return None,
    })
}

/// OKX — venues/okx/errors.rs (Error Code → REST API → Funding).
fn okx(code: &str) -> Option<&'static str> {
    Some(match code {
        "58203" => "в аккаунте OKX не добавлен адрес вывода",
        "58207" => "адреса получателя нет в списке проверенных адресов OKX",
        "58213" | "58222" => "OKX считает адрес получателя недопустимым",
        "58221" => "OKX требует адрес с меткой (address:label)",
        "58254" => "OKX требует подтвердить адрес (цифровая подпись или тест сатоши)",
        "58201" | "58256" | "58258" => DAY_LIMIT,
        "58257" => "исчерпан месячный лимит вывода аккаунта",
        "58232" => "исчерпан лимит вывода уровня верификации аккаунта",
        "58204" | "58217" => RISK,
        "58208" => "к аккаунту OKX не привязан email",
        "58209" | "59504" => SUB,
        "58011" | "58210" | "58230" | "58233" | "58235" | "58240" | "59550" => KYC,
        "58219" => "вывод запрещён 24 часа после смены телефона, email или Google Authenticator",
        "58237" | "58238" => "по местным правилам OKX нужны данные получателя вывода",
        "58239" | "58248" => "по местным правилам вывод через API для аккаунта OKX недоступен",
        "58252" => "вывод запрещён 48 часов после первой операции в TRY",
        "59519" => "функция в аккаунте OKX заморожена",
        // Дополнение t85: код прав у вывода (venues/okx/errors.rs).
        "58004" => "аккаунт OKX заблокирован (Account blocked)",
        _ => return None,
    })
}

/// Gate — venues/gate/errors.rs (Error label list → Wallet).
fn gate(label: &str) -> Option<&'static str> {
    Some(match label {
        "ACCOUNT_EXCEPTION" => "Gate считает аккаунт необычным (Abnormal account)",
        _ => return None,
    })
}

/// XT.com — venues/xt/errors.rs (errorCode).
fn xt(code: &str) -> Option<&'static str> {
    Some(match code {
        "WITHDRAW_002" => "XT считает адрес получателя недопустимым",
        "WITHDRAW_014" => {
            "адрес получателя включён во внутренние переводы XT — для вывода его нужно отключить"
        }
        "WITHDRAW_003" => {
            "низкий уровень безопасности аккаунта XT — нужны две проверки (телефон, email, Google)"
        }
        "WITHDRAW_012" | "WITHDRAW_013" | "WITHDRAW_020" | "WITHDRAW_021" => DAY_LIMIT,
        _ => return None,
    })
}

/// Poloniex — venues/poloniex/errors.rs (Error Codes → Accounts).
fn poloniex(code: &str) -> Option<&'static str> {
    Some(match code {
        "21601" => "аккаунт Poloniex заблокирован — нужна поддержка",
        _ => return None,
    })
}
