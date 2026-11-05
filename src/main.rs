//! Файл: main.rs
//! Что делает: запуск системы — разбор аргументов, база, настройки, журнал, секреты,
//! уведомления (и приём кнопок Telegram), шлюз, ядро и веб-сервер интерфейса (с раздачей собранного интерфейса, если задан --ui-dir).
//! Кто вызывает: операционная система (запуск программы).
//! От чего зависит: все модули. Адаптеры площадок (BingX, DEX в сети BSC) регистрируются здесь.
//! Настройки: свои — нет; аргументы запуска — cli.rs. Распределитель памяти — mimalloc (см. GLOBAL).

use std::process::ExitCode;

use tokio::sync::broadcast;

use arb::api::state::AppState;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::secrets::Vault;
use arb::settings::live::Watch;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::{Journal, Level};

/// Распределитель памяти — mimalloc (https://github.com/microsoft/mimalloc, crate mimalloc: «#[global_allocator]
/// static GLOBAL: MiMalloc = MiMalloc;»). Бинарь для Linux — статический musl (.github/workflows/linux.yml), а malloc musl
/// держит одну общую блокировку на все потоки: на живом 03.11 с MEXC профиль perf — 64 % процессора в спин-блокировке
/// futex (__lock/__unlock musl) и 54 % времени в ядре, 24 потока tokio на 70–100 %. mimalloc — свои кучи на поток, общей
/// блокировки на каждое выделение нет.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Имя файла базы в папке данных.
const DB_FILE: &str = "arb.sqlite";

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt::init();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Собирает и запускает все компоненты.
/// Ошибки: неверные аргументы, нет доступа к папке данных, база, занят адрес.
async fn run() -> Result<(), String> {
    let args = arb::cli::parse(std::env::args().skip(1))?;
    std::fs::create_dir_all(&args.data_dir)
        .map_err(|e| format!("Папка данных {:?}: {e}", args.data_dir))?;
    let db = Db::open(&args.data_dir.join(DB_FILE)).map_err(|e| e.to_string())?;

    let (changes, _) = broadcast::channel(args.live_buffer);
    let (journal_live, _) = broadcast::channel(args.live_buffer);
    let journal = Journal::new(db.clone(), journal_live.clone());
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .map_err(|e| e.to_string())?;
    let vault = Vault::locked(arb::secrets::file::path_in(&args.data_dir));
    let http = reqwest::Client::builder()
        // Редиректы выключены: в адресе запроса может быть токен, его нельзя отдавать другому адресу.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let notifier = Notifier {
        http: http.clone(),
        settings: settings.clone(),
        vault: vault.clone(),
        journal: journal.clone(),
    };

    // Изменения настроек — в журнал и в Telegram (если событие включено).
    tokio::spawn(arb::notify::settings_watch::run(
        Watch::new(changes.subscribe(), vec![]),
        notifier.clone(),
    ));

    // Адаптеры площадок: создаются здесь и регистрируются в шлюзе; ядро видит их только через шлюз.
    let registry = Registry::new();
    let raw = arb::storage::raw_log::RawLog::start(args.data_dir.clone(), settings.clone());
    let bingx_ctx = arb::venues::bingx::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: http.clone(),
        journal: journal.clone(),
        raw: raw.clone(),
        taps: Default::default(),
    };
    registry
        .register(std::sync::Arc::new(arb::venues::bingx::Bingx::new(
            bingx_ctx,
        )))
        .await;
    // Биржа MEXC (решение 107): тот же контракт, свои настройки venue.* [mexc] и секреты mexc.*.
    let mexc_ctx = arb::venues::mexc::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: http.clone(),
        journal: journal.clone(),
        raw: raw.clone(),
        taps: Default::default(),
    };
    registry
        .register(std::sync::Arc::new(arb::venues::mexc::Mexc::new(mexc_ctx)))
        .await;
    // Биржа Gate (решение 135): тот же контракт, свои настройки venue.* [gate] и секреты gate.*.
    let gate_ctx = arb::venues::gate::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: http.clone(),
        journal: journal.clone(),
        raw: raw.clone(),
        taps: Default::default(),
    };
    registry
        .register(std::sync::Arc::new(arb::venues::gate::Gate::new(gate_ctx)))
        .await;
    // Биржа Bybit (решение 136): API V5, только спот; свои настройки venue.* [bybit] и секреты bybit.*.
    let bybit_ctx = arb::venues::bybit::ctx::Ctx::new(
        settings.clone(),
        vault.clone(),
        http.clone(),
        journal.clone(),
        raw.clone(),
    );
    registry
        .register(std::sync::Arc::new(arb::venues::bybit::Bybit::new(
            bybit_ctx,
        )))
        .await;
    // Биржа KuCoin (решение 137): тот же контракт, свои настройки venue.* [kucoin] и секреты kucoin.*.
    let kucoin_ctx = arb::venues::kucoin::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: http.clone(),
        journal: journal.clone(),
        raw: raw.clone(),
        taps: Default::default(),
    };
    registry
        .register(std::sync::Arc::new(arb::venues::kucoin::Kucoin::new(
            kucoin_ctx,
        )))
        .await;
    // Биржа Binance (решение 160): спот API и Wallet SAPI; свои настройки venue.* [binance] и секреты binance.*.
    let binance_ctx = arb::venues::binance::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: http.clone(),
        journal: journal.clone(),
        raw: raw.clone(),
        taps: Default::default(),
    };
    registry
        .register(std::sync::Arc::new(arb::venues::binance::Binance::new(
            binance_ctx,
        )))
        .await;
    // Биржа Bitget (решение 162): Classic API v2, только спот; свои настройки venue.* [bitget] и секреты bitget.*.
    let bitget_ctx = arb::venues::bitget::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: http.clone(),
        journal: journal.clone(),
        raw: raw.clone(),
    };
    registry
        .register(std::sync::Arc::new(arb::venues::bitget::Bitget::new(
            bitget_ctx,
        )))
        .await;
    // Биржа OKX (решение 161): API v5, только спот; свои настройки venue.* [okx] и секреты okx.*.
    let okx_ctx = arb::venues::okx::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: http.clone(),
        journal: journal.clone(),
        raw: raw.clone(),
    };
    registry
        .register(std::sync::Arc::new(arb::venues::okx::Okx::new(okx_ctx)))
        .await;
    // Биржа Poloniex (решение 169): Spot API, только спот; свои настройки venue.* [poloniex] и секреты poloniex.*.
    let poloniex_ctx = arb::venues::poloniex::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: http.clone(),
        journal: journal.clone(),
        raw: raw.clone(),
    };
    registry
        .register(std::sync::Arc::new(arb::venues::poloniex::Poloniex::new(
            poloniex_ctx,
        )))
        .await;
    // Площадка DEX — bsc_dex (решение 91): сигналы, цены и подбор размера всегда с нашего узла (WebSocket пулов) и
    // математики пулов. Исполнение свопа по настройке exec.via_kyber [bsc_dex] делегируется агрегатору KyberSwap
    // (внутри bsc_dex), иначе — собственный сборщик. Код исполнения kyberswap как площадка-цены не регистрируется.
    let dex_ctx = arb::venues::bsc_dex::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: http.clone(),
        journal: journal.clone(),
        raw,
        pace: Default::default(),
        failover: Default::default(),
        account: None,
        index: Default::default(),
    };
    // Решение 132, пункт 5: перенос прежнего единственного кошелька — один раз, после разблокировки секретов
    // (api/secrets.rs): кошелёк «BingX» с тем же секретом, исполнитель — значение exec.executor_address [bsc_dex],
    // строки маппинга bingx и «пробы». Деньги не переводятся.
    settings
        .accounts()
        .configure_legacy(arb::settings::accounts::migrate::LegacyCfg {
            name: "BingX".into(),
            secret: arb::venues::bsc_dex::ctx::SECRET_WALLET_KEY.into(),
            queue_key: arb::venues::bsc_dex::ctx::ID.into(),
            executor_setting: "exec.executor_address".into(),
            executor_scope: arb::settings::scope::Scope::Venue(
                arb::venues::bsc_dex::ctx::ID.into(),
            ),
            rows: vec![
                arb::venues::bingx::ctx::ID.into(),
                arb::settings::accounts::ROW_PROBE.into(),
            ],
        });
    let dex = arb::venues::bsc_dex::BscDex::new(dex_ctx);
    // Изменение промежуточных монет (решение 52) — пересборка подписки DEX на лету.
    dex.watch_settings(changes.subscribe());
    registry.register(std::sync::Arc::new(dex)).await;

    // Ядро: работает с площадками только через шлюз; интерфейс читает его состояние через AppState.core.
    let core = arb::core::Core::start(arb::core::deps::Deps {
        gw: registry.clone(),
        settings: settings.clone(),
        journal: journal.clone(),
        notifier: notifier.clone(),
        db: db.clone(),
        changes: changes.clone(),
        buffer: args.live_buffer,
        data_dir: args.data_dir.clone(),
    })
    .await;

    // Кнопки и команды Telegram → команды ядру (Core::command, решения 156, 157). wake — сигнал «секреты
    // изменились» от api/secrets.rs: приём начинает заново с актуальным токеном.
    let secrets_wake = std::sync::Arc::new(tokio::sync::Notify::new());
    // Память дубля «Ручного управления» — общая у отправки (mirror.rs) и приёма нажатий (updates.rs).
    let mirror_state = arb::notify::mirror_state::Shared::default();
    tokio::spawn(arb::notify::mirror::run(
        arb::notify::mirror::Mirror {
            core: core.clone(),
            notifier: notifier.clone(),
            state: mirror_state.clone(),
        },
        Watch::new(
            changes.subscribe(),
            arb::notify::updates::WATCH_PREFIXES
                .iter()
                .map(|p| p.to_string())
                .collect(),
        ),
    ));
    tokio::spawn(arb::notify::updates::run(
        arb::notify::updates::Updates {
            http: http.clone(),
            settings: settings.clone(),
            vault: vault.clone(),
            journal: journal.clone(),
            core: core.clone(),
            state: mirror_state,
            wake: secrets_wake.clone(),
        },
        Watch::new(
            changes.subscribe(),
            arb::notify::updates::WATCH_PREFIXES
                .iter()
                .map(|p| p.to_string())
                .collect(),
        ),
    ));

    let state = AppState {
        db,
        settings,
        journal: journal.clone(),
        registry,
        vault,
        notifier: notifier.clone(),
        http,
        changes,
        journal_live,
        core,
        secrets_wake,
    };
    let guard = arb::api::guard::Guard::new(&args.listen.to_string(), &args.allow_origins);
    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .map_err(|e| format!("Не удалось занять адрес {}: {e}", args.listen))?;

    let started = match &args.ui_dir {
        Some(_) => format!("Система запущена, интерфейс: http://{}", args.listen),
        None => format!(
            "Система запущена без раздачи интерфейса (--ui-dir не задан), API: http://{}",
            args.listen
        ),
    };
    tracing::info!("{started}");
    let _ = journal
        .write(Level::Info, "system.started", None, started.clone(), None)
        .await;
    notifier.send("system.started", None, &started).await;

    arb::api::server::serve(listener, state, guard, args.ui_dir)
        .await
        .map_err(|e| format!("Веб-сервер остановился: {e}"))
}
