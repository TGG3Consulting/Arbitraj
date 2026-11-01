//! Файл: settings/catalog/kyber.rs
//! Что делает: настройки адаптера DEX через агрегатор KyberSwap (площадка `kyberswap`, решение 91) — адрес API,
//! сеть, идентификатор клиента, частота поллинга квот, допуск проскальзывания, срок сборки транзакции, узел BSC для
//! исполнения, газ, ожидание квитанции и переключатель «вместо bsc_dex». Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «KyberSwap (агрегатор)»).
//!
//! Все поля — строго по официальной документации агрегатора KyberSwap (EVM-свопы):
//! https://docs.kyberswap.com/kyberswap-solutions/kyberswap-aggregator/aggregator-api-specification/evm-swaps
//! (эндпоинты /{chain}/api/v1/routes и /{chain}/api/v1/route/build, заголовок X-Client-Id, slippageTolerance 0–2000 bps,
//! deadline в секундах Unix). Узел BSC — как у bsc_dex (eth_* execution-apis), ключ провайдера в адресе как {key}.

use super::{G, V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки адаптера KyberSwap (агрегатор). Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static KYBER: &[SettingDef] = &[
    d("KyberSwap (агрегатор)", "kyber.base_url", K::Url, L::None, "URL", G, "База API агрегатора (без сети и пути): прямые эндпоинты https://aggregator-api.kyberswap.com или шлюз https://api.kyberswap.com/swap (выше лимиты, нужен X-Api-Key от Business Development). Путь /{chain}/api/v1/... добавляется адаптером"),
    d("KyberSwap (агрегатор)", "kyber.chain", K::Text, L::None, "сеть", G, "Сегмент сети в пути запроса (документация: имя сети, не chainId). BSC — bsc"),
    d("KyberSwap (агрегатор)", "kyber.client_id", K::Text, L::None, "идентификатор", G, "Заголовок X-Client-Id — свой идентификатор приложения (без него лимиты строже; регистрация и платный ключ не нужны). Не задан — запросы без заголовка").optional(),
    d("KyberSwap (агрегатор)", "kyber.quote_poll_ms", K::Integer, L::Positive, "мс", G, "Частота опроса квот агрегатора для «Живого экрана» (у агрегатора нет потока цен — цены берутся поллингом GET /routes по активным монетам). Меньше — свежее цены и больше запросов к API"),
    d("KyberSwap (агрегатор)", "kyber.quote_concurrency", K::Integer, L::Positive, "запросов", G, "Сколько запросов /routes слать одновременно в одном цикле поллинга (монеты обрабатываются пачками этого размера). Держит нагрузку на API в пределах; совет — по лимиту тарифа"),
    d("KyberSwap (агрегатор)", "kyber.probe_usdt", K::Decimal, L::Positive, "USDT", G, "Пробный объём котировки для «Живого экрана»: сколько USDT закладывать в GET /routes при опросе цены монеты (цена зависит от объёма — это объём, на котором показывается цена DEX в живом потоке). На расчёт кандидата и сделку не влияет — там объём считает ядро"),
    d("KyberSwap (агрегатор)", "kyber.slippage_bps", K::Integer, L::Positive, "bps", G, "Допуск проскальзывания при сборке транзакции (POST /route/build, поле slippageTolerance, единицы bps). Документация: 0–2000 (0–20%); из него роутер считает минимально допустимый выход свопа на цепочке").max(2000),
    d("KyberSwap (агрегатор)", "kyber.deadline_s", K::Integer, L::Positive, "секунды", G, "Срок жизни транзакции обмена (POST /route/build, поле deadline, Unix-секунды = сейчас + это): позже роутер отвергает своп"),
    d("KyberSwap (агрегатор)", "kyber.http_timeout_ms", K::Integer, L::Positive, "мс", G, "Сколько ждать заголовков ответа API агрегатора и узла BSC по HTTP (тело дочитывается по http.body_idle_ms / http.total_ms / http.max_body_bytes — как у остальных площадок)"),
    d("KyberSwap (агрегатор)", "kyber.node_http_url", K::Url, L::None, "URL", G, "HTTP узла BSC для ИСПОЛНЕНИЯ (цены — из агрегатора, узел для цен не нужен): eth_call, eth_estimateGas, eth_gasPrice, eth_sendRawTransaction, eth_getTransactionReceipt. Ключ провайдера в адресе — как {key}, сам ключ — секрет kyber.node_http_key"),
    d("KyberSwap (агрегатор)", "kyber.gas_price_markup_pct", K::Decimal, L::Percent, "%", G, "Надбавка к eth_gasPrice при отправке свопа и перевода (быстрее попасть в блок). 0 — без надбавки"),
    d("KyberSwap (агрегатор)", "kyber.gas_limit_margin_pct", K::Decimal, L::Percent, "%", G, "Запас к оценке eth_estimateGas для лимита газа транзакции (чтобы не упереться в лимит на граничных путях)"),
    d("KyberSwap (агрегатор)", "kyber.receipt_poll_ms", K::Integer, L::Positive, "мс", G, "Как часто опрашивать eth_getTransactionReceipt после отправки транзакции (своп, approve, перевод), пока она не попадёт в блок"),
    d("KyberSwap (агрегатор)", "kyber.receipt_timeout_ms", K::Integer, L::Positive, "мс", G, "Сколько ждать квитанции транзакции. Дольше — «не попала в блок» (TxNotIncluded), повторов нет (стартовое правило Тиграна: отправлено и не исполнилось — больше не пробуем)"),
    d("KyberSwap (агрегатор)", "kyber.deposit_confirmations", K::Integer, L::Positive, "блоков", G, "Сколько блоков после перевода на кошелёк ждать до статуса «зачислено» (deposit_status): раньше — «в ожидании»"),
    d("KyberSwap (агрегатор)", "kyber.assume_zero_transfer_tax", K::Bool, L::None, "да/нет", V, "Решение 91: явное разрешение человека считать токены площадки kyberswap не берущими налог на обычный перевод (кошелёк → биржа). Да — адаптер шлёт CoinCheck с нулевыми налогами (налог свопа уже внутри котировки маршрута агрегатора; отдельный налог перевода принят равным нулю), и «Живой экран» показывает цены и спреды DEX, авто-сделки допускаются. Нет или не задана — налоги по монетам площадки считаются непроверенными: направления видны, но без цен, сделок нет (агрегатор не измеряет налог перевода). Включать только для выверенного набора монет без налога на перевод").optional(),
];
