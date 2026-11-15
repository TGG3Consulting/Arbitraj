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
//! exec/executor/code.rs (runtime, runtime_v1, Version).
//! Решение 284: то же правило для вида шага, которого нет в исполнителе v1 (NOMI_STABLE = 12 — только v2): у кошелька
//! v1 — пулы DEX без пути исполнения с причиной «переразверните исполнитель»; версия не сверена — «не сверен».
//! Настройки: dex.v3_swap_callback [DEX] (необязательная).

use std::collections::HashMap;
use std::sync::Mutex;

use super::super::ctx::Ctx;
use super::super::exec::executor::code::{self, Version};
use super::exec_kind::ExecKind;
use super::families::Family;
use super::registry_key;

/// Имя настройки.
pub const SETTING: &str = "dex.v3_swap_callback";

/// Память итогов: ключ (версия, сигнатура) → None — вызов есть, Some — причина.
type Seen = HashMap<(Version, String), Option<String>>;

/// Итог поиска по ключу (версия, сигнатура) — артефакты неизменны до перезапуска.
static SEEN: Mutex<Option<Seen>> = Mutex::new(None);

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
        Version::V2 => code::runtime(),
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
    let Ok(w) = ctx.wallet() else { return Ok(None) };
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
    let mut g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashMap::new)
        .entry((v, sig.clone()))
        .or_insert_with(|| check(v, &sig))
        .clone()
}

/// Решение 284: вид шага есть только в исполнителе v2 (номер вида добавлен в contracts/ArbExecutor.sol после v1:
/// NOMI_STABLE = 12; у v1 диспетчер _step откатывает «kind»). Принимает: вид шага. Ошибок нет.
pub fn v2_only(k: ExecKind) -> bool {
    matches!(k, ExecKind::NomiStable | ExecKind::Metric)
}

/// Нет ли у исполнителя кошелька окружения вида шага этой DEX (решение 284). Принимает: окружение, вид. Возвращает:
/// None — вид шага есть у обеих версий, кошелёк без исполнителя (решает executor_set) или исполнитель v2; Some —
/// причина (исполнитель v1 / версия не сверена). Ошибок нет.
pub fn step_missing(ctx: &Ctx, f: &Family) -> Option<String> {
    let k = f.executor_kind.filter(|k| v2_only(*k))?;
    let what = format!("шаг исполнителя {}", k.name());
    match wallet_version(ctx, &what) {
        Ok(Some(Version::V1)) => Some(format!(
            "исполнитель v1 не знает {what} (вид шага есть только в v2) — переразверните исполнитель кошелька \
             (экран «Маппинг кошельков»)"
        )),
        Ok(_) => None,
        Err(why) => Some(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Селектор squadV3SwapCallback = 0xc4e21d86 (SquadV3Pool.sol L794): в артефакте v1 его нет, в v2 (t114) есть;
    /// pancakeV3SwapCallback / uniswapV3SwapCallback — в обоих (тот же поиск PUSH4, реальные артефакты contracts/out/).
    #[test]
    fn squad_callback_by_executor_version() {
        let squad = registry_key::selector("squadV3SwapCallback(int256,int256,bytes)");
        assert_eq!(squad.to_vec(), vec![0xc4, 0xe2, 0x1d, 0x86]);
        let (v1, v2) = (code::runtime_v1().unwrap(), code::runtime().unwrap());
        assert!(!has_selector(&v1, &squad));
        assert!(has_selector(&v2, &squad));
        for sig in [
            "pancakeV3SwapCallback(int256,int256,bytes)",
            "uniswapV3SwapCallback(int256,int256,bytes)",
        ] {
            let sel = registry_key::selector(sig);
            assert!(has_selector(&v1, &sel) && has_selector(&v2, &sel), "{sig}");
            assert!(check(Version::V1, sig).is_none() && check(Version::V2, sig).is_none());
        }
        let s = "squadV3SwapCallback(int256,int256,bytes)";
        assert!(
            check(Version::V1, s).is_some_and(|w| w.contains("v1") && w.contains("0xc4e21d86"))
        );
        assert!(check(Version::V2, s).is_none());
    }
}
