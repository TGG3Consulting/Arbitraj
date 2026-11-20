//! Файл: venues/bsc_dex/pools/exec_callback.rs
//! Что делает: ревью 281, В4 — обратный вызов обмена пула V3-форка, которого может не быть у контракта-исполнителя.
//! Пул SquadSwap V3 в swap зовёт у вызывающего `squadV3SwapCallback(int256,int256,bytes)` (0xc4e21d86, SquadV3Pool.sol
//! L794, L800), а не uniswapV3SwapCallback / pancakeV3SwapCallback. Необязательная настройка DEX dex.v3_swap_callback —
//! сигнатура обратного вызова, который зовёт пул (Squad: `squadV3SwapCallback(int256,int256,bytes)`). Задана — её
//! селектор (первые 4 байта keccak256 сигнатуры) ищется как PUSH4 селектора (0x63 + 4 байта — так диспетчер solc
//! сравнивает селектор вызова) в коде в сети ТОЙ версии исполнителя, что сверена у кошелька окружения (решение 272:
//! exec/executor/onchain.rs пишет executor_version кошелька по коду по адресу; задача t114): v2 — артефакт
//! contracts/out/ArbExecutor.bin-runtime (code::runtime), v1 — contracts/out/v1/ArbExecutor.bin-runtime
//! (code::runtime_v1). Версия не сверена — пути исполнения нет с причиной «исполнитель не сверен» (не исполняем
//! вслепую: у кошелька может стоять v1, а селектор есть только в v2 — пул позвал бы несуществующий вызов, откат, и
//! монета ложно ушла бы в чёрный список). Кошелёк без исполнителя — проверки здесь нет (это решает executor_set,
//! exec_kind.rs). Вызова нет — пулы DEX без пути исполнения через исполнитель (exec_kind.rs via_executor = нет): в
//! маршруты и бандлы не идут, проба налогов через исполнитель для них не запускается (NoRoute: «не проверено», монета
//! не голосуется, check/probe_exec.rs). Факт артефактов: в v1 0xc4e21d86 нет; в v2 есть (задача t114 добавила
//! squadV3SwapCallback в contracts/ArbExecutor.sol). Не задана настройка — проверки нет (как раньше).
//! Кто вызывает: pools/exec_kind.rs (via_executor, describe), check/probe_exec.rs (no_path).
//! От чего зависит: ctx.rs, ctx_wallet.rs (кошелёк окружения), families.rs, registry_key.rs (selector),
//! exec/executor/code.rs (runtime, runtime_v1, Version), exec/executor/deploy_gate.rs (need_v3 — решение 399).
//! Решение 284: то же правило для вида шага, которого нет в исполнителе v1 (NOMI_STABLE = 12 — только v2): у кошелька
//! v1 — пулы DEX без пути исполнения с причиной «переразверните исполнитель»; версия не сверена — «не сверен».
//! Решение 395 (t181): шаги пулов внутри менеджеров — UNI_V4 = 19, INF_CL = 20, INF_BIN = 21 — только у исполнителя v3
//! (since): у кошелька v1 / v2 — пулы Uniswap V4 / PancakeSwap Infinity без пути ЧЕРЕЗ ИСПОЛНИТЕЛЬ (свой Universal Router у
//! них остаётся — family.tradable); артефакт версии для поиска селектора: v1 — out/v1/, v2 — out/v2/, v3 — out/.
//! Решение 409 (t194): память итогов SEEN — снимок за ArcSwap (чтение без замка на горячем пути котировки).
//! Настройки: dex.v3_swap_callback [DEX] (необязательная).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use arc_swap::ArcSwap;

use super::super::ctx::Ctx;
use super::super::exec::executor::code::{self, Version};
use super::super::exec::executor::deploy_gate;
use super::exec_kind::ExecKind;
use super::families::Family;
use super::registry_key;

/// Имя настройки.
pub const SETTING: &str = "dex.v3_swap_callback";

/// Решения 440, 441, 456: исполнитель v4 (шаги FLUID = 22, BALANCER_V3 = 23, LB20 = 24, EKUBO = 25) есть только в
/// исходнике contracts/ArbExecutor.sol — артефакты contracts/out/ ещё v3; кошелька v4 быть не может, пока артефакты не
/// пересобраны (contracts/build.sh) и исполнитель не переразвёрнут.
pub const V4_TODO: &str =
    "нужен исполнитель v4: пересоберите contracts/build.sh и переразверните (решение 456)";

/// Память итогов: версия → сигнатура → None — вызов есть, Some — причина.
type Seen = HashMap<Version, HashMap<String, Option<String>>>;

/// Итог поиска по ключу (версия, сигнатура) — артефакты неизменны до перезапуска. Решение 409 (t194): снимок за
/// ArcSwap — горячий путь котировки (usable на каждый хоп через exec_kind::via_executor) читает без замка и без
/// выделения строки; новая запись — под SEEN_WRITE.
static SEEN: LazyLock<ArcSwap<Seen>> = LazyLock::new(|| ArcSwap::from_pointee(HashMap::new()));
/// Писатели памяти SEEN — по одному (поиск в артефакте — один раз на ключ).
static SEEN_WRITE: Mutex<()> = Mutex::new(());

/// Есть ли PUSH4 селектора в коде (0x63 и 4 байта селектора подряд). Принимает: код, селектор. Ошибок нет.
pub fn has_selector(runtime: &[u8], sel: &[u8]) -> bool {
    let mut want = vec![0x63u8];
    want.extend_from_slice(sel);
    runtime.windows(want.len()).any(|w| w == want.as_slice())
}

/// Причина «исполнитель этой версии не поддерживает обратный вызов» (без памяти). Принимает: версию, сигнатуру.
/// Возвращает: None — селектор в артефакте версии есть; Some — причина. Ошибок нет: артефакт не сверен по хешу — тоже
/// причина (не исполняем вслепую).
pub fn check(v: Version, sig: &str) -> Option<String> {
    let sel = registry_key::selector(sig);
    let rt = match v {
        Version::V1 => code::runtime_v1(),
        Version::V2 => code::runtime_v2(),
        Version::V3 => code::runtime_v3(),
        Version::V4 => code::runtime(),
    };
    let ver = v.label();
    match rt {
        Ok(rt) if has_selector(&rt, &sel) => None,
        Ok(_) => Some(format!(
            "исполнитель {ver} не поддерживает обратный вызов {sig} (селектор {sel}) — нужен исполнитель новой версии \
             (переразверните исполнитель кошелька, экран «Маппинг кошельков»)"
        )),
        Err(why) => Some(format!(
            "код исполнителя {ver} не сверен ({why}) — обратный вызов {sig} не проверить"
        )),
    }
}

/// Версия исполнителя кошелька окружения. Принимает: окружение, что проверяем (текст причины). Возвращает: Ok(None) — кошелька или исполнителя нет (проверку решает
/// executor_set); Ok(Some(версия)) — сверена; Err(причина) — исполнитель есть, версия не сверена. Ошибок иных нет.
fn wallet_version(ctx: &Ctx, what: &str) -> Result<Option<Version>, String> {
    // Решение 401 (ревью t186, Н1): снимок кошельков без замка и без копии записи.
    let acc = ctx.settings.accounts();
    // Решение 432: исполнитель этой площадки DEX (её сети).
    acc.with_wallet_at(ctx.place.id, ctx.account.as_deref(), |found| {
        let Some(w) = found else { return Ok(None) };
        let Some(x) = w.executor.as_deref() else {
            return Ok(None);
        };
        match w.executor_version.as_deref().and_then(Version::parse) {
            Some(v) => Ok(Some(v)),
            None => Err(format!(
                "исполнитель {x} кошелька «{}» не сверен — {what} не проверить (сверка — при первой сделке или \
                 пробе через исполнитель, или кнопкой «Проверить подключение» исполнителя)",
                w.name
            )),
        }
    })
}

/// Решение 401 (ревью t186, Н1): то же, что `missing(ctx, f).is_none()` при заданном исполнителе, — для горячего пути
/// котировки (exec_kind.rs via_executor): без чтения записи кошелька и без строк; версия — из снимка (Ctx::exec_info).
/// Вид шага с v2 / v3 — нужна сверенная версия не ниже since(вид); задана dex.v3_swap_callback — её селектор в коде
/// сверенной версии (память SEEN). Принимает: окружение, вид, сверенную версию (None — не сверена). Ошибок нет.
pub fn usable(ctx: &Ctx, f: &Family, ver: Option<Version>) -> bool {
    if let Some(k) = f.executor_kind.filter(|k| since(*k) > Version::V1)
        && !ver.is_some_and(|v| v >= since(k))
    {
        return false;
    }
    let Ok(text) = ctx.dex_text(SETTING, f.id) else {
        return true;
    };
    let sig = text.trim();
    if sig.is_empty() {
        return true;
    }
    match ver {
        Some(v) => seen_with(v, sig, |r| r.is_none()),
        None => false,
    }
}

/// Итог поиска селектора по памяти SEEN (артефакты неизменны до перезапуска), переданный в `f`. Решение 409: чтение
/// снимка без замка; нет записи — поиск в артефакте (check) и новый снимок под SEEN_WRITE. Принимает: версию,
/// сигнатуру, f (итог: None — вызов есть; Some — причина). Возвращает: итог f. Ошибок нет.
fn seen_with<R>(v: Version, sig: &str, f: impl FnOnce(&Option<String>) -> R) -> R {
    if let Some(r) = SEEN.load().get(&v).and_then(|m| m.get(sig)) {
        return f(r);
    }
    let _turn = SEEN_WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let cur = SEEN.load_full();
    if let Some(r) = cur.get(&v).and_then(|m| m.get(sig)) {
        return f(r);
    }
    let r = check(v, sig);
    let out = f(&r);
    let mut next = (*cur).clone();
    next.entry(v).or_default().insert(sig.to_string(), r);
    SEEN.store(Arc::new(next));
    out
}

/// Нет ли у исполнителя кошелька окружения обратного вызова, который зовёт пул этой DEX. Принимает: окружение, вид.
/// Возвращает: None — настройка не задана, у кошелька нет исполнителя или вызов есть в коде сверенной версии;
/// Some — причина (вызова нет в этой версии / версия не сверена). Ошибок нет.
pub fn missing(ctx: &Ctx, f: &Family) -> Option<String> {
    // Решение 284: вид шага, которого нет у исполнителя v1, — та же причина «без пути исполнения».
    if let Some(why) = step_missing(ctx, f) {
        return Some(why);
    }
    let sig = ctx.dex_text(SETTING, f.id).ok()?.trim().to_string();
    if sig.is_empty() {
        return None;
    }
    let v = match wallet_version(ctx, &format!("обратный вызов {sig}")) {
        Ok(Some(v)) => v,
        Ok(None) => return None,
        Err(why) => return Some(why),
    };
    seen_with(v, &sig, |r| r.clone())
}

/// С какой версии исполнителя есть вид шага (у более ранней диспетчер _step откатывает «kind»). Решение 284: v2 —
/// NOMI_STABLE = 12, METRIC = 13, IZI = 14 — решение 286, KYBER_DMM = 15 — решение 287, SMARDEX = 16 — решение 288,
/// BALANCER = 17 — решение 289, LB = 18 — решение 290. Решение 395 (t181): v3 — UNI_V4 = 19, INF_CL = 20, INF_BIN = 21.
/// Решение 440: v4 — FLUID = 22. Остальные — с v1. Принимает: вид шага. Ошибок нет.
pub fn since(k: ExecKind) -> Version {
    match k {
        ExecKind::NomiStable
        | ExecKind::Metric
        | ExecKind::Izi
        | ExecKind::KyberDmm
        | ExecKind::Smardex
        | ExecKind::Balancer
        | ExecKind::Lb => Version::V2,
        ExecKind::UniV4 | ExecKind::InfCl | ExecKind::InfBin => Version::V3,
        ExecKind::Fluid => Version::V4,
        // Решение 441: BALANCER_V3 = 23 — с v4.
        ExecKind::BalancerV3 => Version::V4,
        _ => Version::V1,
    }
}

/// Вид шага появился в v2 (решения 284–290: есть у v2 и v3, нет у v1) — прежнее имя, им пользуются тесты видов. Ошибок нет.
pub fn v2_only(k: ExecKind) -> bool {
    since(k) == Version::V2
}

/// Причина «у исполнителя версии v нет вида шага k» (решения 284, 395): None — есть. Решение 399 (Н2): вид с v3 —
/// «что делать» по deploy_gate::need_v3 (пока v3 не принят на живых пулах — без призыва переразвернуть). Принимает:
/// вид, версию. Ошибок нет.
pub fn step_absent(k: ExecKind, v: Version) -> Option<String> {
    let need = since(k);
    (v < need).then(|| {
        let todo = if need >= Version::V4 {
            V4_TODO
        } else if need >= Version::V3 {
            deploy_gate::need_v3()
        } else {
            "переразверните исполнитель кошелька (экран «Маппинг кошельков»)"
        };
        format!(
            "исполнитель {} не знает шаг исполнителя {} (вид шага есть с {}) — {todo}",
            v.label(),
            k.name(),
            need.label()
        )
    })
}

/// Нет ли у исполнителя кошелька окружения вида шага этой DEX (решения 284, 395). Принимает: окружение, вид.
/// Возвращает: None — вид шага есть с v1, кошелёк без исполнителя (решает executor_set) или сверенная версия не ниже
/// since(вид); Some — причина (версия ниже / версия не сверена). Ошибок нет.
pub fn step_missing(ctx: &Ctx, f: &Family) -> Option<String> {
    let k = f.executor_kind.filter(|k| since(*k) > Version::V1)?;
    let what = format!("шаг исполнителя {}", k.name());
    match wallet_version(ctx, &what) {
        Ok(Some(v)) => step_absent(k, v),
        Ok(None) => None,
        Err(why) => Some(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Селектор squadV3SwapCallback = 0xc4e21d86 (SquadV3Pool.sol L794): в артефакте v1 его нет, в v2 (t114) и v3
    /// (решение 395, текущая сборка contracts/out/) есть; pancakeV3SwapCallback / uniswapV3SwapCallback — во всех (тот же
    /// поиск PUSH4, реальные артефакты contracts/out/v1/, out/v2/, out/). Ревью t181 (Н7): v2 — out/v2/, v3 — out/.
    #[test]
    fn squad_callback_by_executor_version() {
        let squad = registry_key::selector("squadV3SwapCallback(int256,int256,bytes)");
        assert_eq!(squad.to_vec(), vec![0xc4, 0xe2, 0x1d, 0x86]);
        let v1 = code::runtime_v1().unwrap();
        let v2 = code::runtime_v2().unwrap();
        let v3 = code::runtime().unwrap();
        assert!(!has_selector(&v1, &squad));
        assert!(has_selector(&v2, &squad) && has_selector(&v3, &squad));
        for sig in [
            "pancakeV3SwapCallback(int256,int256,bytes)",
            "uniswapV3SwapCallback(int256,int256,bytes)",
        ] {
            let sel = registry_key::selector(sig);
            assert!(
                has_selector(&v1, &sel) && has_selector(&v2, &sel) && has_selector(&v3, &sel),
                "{sig}"
            );
            for v in [Version::V1, Version::V2, Version::V3] {
                assert!(check(v, sig).is_none(), "{sig} {v:?}");
            }
        }
        let s = "squadV3SwapCallback(int256,int256,bytes)";
        assert!(
            check(Version::V1, s).is_some_and(|w| w.contains("v1") && w.contains("0xc4e21d86"))
        );
        assert!(check(Version::V2, s).is_none() && check(Version::V3, s).is_none());
    }
}
