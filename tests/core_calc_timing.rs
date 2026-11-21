//! Файл: tests/core_calc_timing.rs
//! Что делает: проверяет замер этапов расчёта кандидата (решение 129, core/calc_timing.rs, core/calc_report.rs):
//! части одного имени складываются, запросы к узлу (gateway/calc_meter.rs) попадают в этап, где они были, а отказ по
//! сроку называет этап и часть, на которых расчёт стоит, и даёт ключ повтора без чисел.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{calc_timing, calc_report}, arb::gateway::calc_meter. Данных площадок нет — проверяется
//! только учёт времени (длительности — настоящие паузы tokio).

use std::time::{Duration, Instant};

use arb::core::calc_report::{report, timeout_reason};
use arb::core::calc_timing::{Timing, scope, stage, stage_sync};
use arb::gateway::calc_meter::{self, Source};

/// Расчёт, похожий на profit::decide: контекст, бюджет, подбор объёма (части — котировки A/B), затем часть,
/// которая не кончается (её обрывает срок).
async fn calc() {
    stage_sync(|| "контекст направления".into(), || ());
    stage(|| "бюджет".into(), async {}).await;
    stage(|| "подбор объёма".into(), async {
        for _ in 0..3 {
            stage(|| "котировка A bsc_dex".into(), async {
                // Запрос к узлу: 2 вызова пачкой, очередь 1 мс, ответ 3 мс.
                calc_meter::record(
                    Source::Node,
                    2,
                    Duration::from_millis(1),
                    Duration::from_millis(3),
                );
                tokio::time::sleep(Duration::from_millis(2)).await;
            })
            .await;
            stage(|| "котировка B bingx".into(), async {}).await;
        }
        stage(
            || "котировка A bsc_dex (зависла)".into(),
            std::future::pending::<()>(),
        )
        .await;
    })
    .await;
}

#[tokio::test]
async fn timeout_names_stuck_stage_and_sums_parts() {
    let t = Timing::new(Instant::now());
    let work = calc_meter::scope(t.meter.clone(), scope(t.clone(), calc()));
    assert!(
        tokio::time::timeout(Duration::from_millis(60), work)
            .await
            .is_err(),
        "расчёт не кончается — срок"
    );
    let r = report(&t);
    assert_eq!(
        r.stuck,
        Some((
            "подбор объёма".to_string(),
            Some("котировка A bsc_dex (зависла)".to_string())
        ))
    );
    let names: Vec<&str> = r.stages.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["контекст направления", "бюджет", "подбор объёма"]);
    let sizing = &r.stages[2];
    assert!(sizing.open, "подбор объёма ещё идёт");
    let a = sizing
        .parts
        .iter()
        .find(|p| p.name == "котировка A bsc_dex")
        .unwrap();
    assert_eq!(a.count, 3, "части одного имени складываются");
    assert_eq!(a.node_calls, 6, "3 запроса по 2 вызова");
    assert_eq!(a.node_wait_ms, 3.0);
    assert_eq!(a.node_ms, 9.0);
    assert!(a.ms >= 6.0, "три паузы по 2 мс: {}", a.ms);
    assert_eq!(r.node_calls, 6);
    let stuck = sizing.parts.iter().find(|p| p.open).unwrap();
    assert!(
        stuck.ms >= 40.0,
        "зависшая часть идёт почти весь срок: {}",
        stuck.ms
    );

    let (reason, key) = timeout_reason(60, &r);
    assert!(
        reason.starts_with("расчёт не уложился в срок 60 мс: стоит на этапе «подбор объёма» → котировка A bsc_dex (зависла)"),
        "{reason}"
    );
    assert!(reason.contains("котировка A bsc_dex"), "{reason}");
    assert!(reason.contains("×3"), "{reason}");
    assert_eq!(
        key,
        "расчёт не уложился в срок 60 мс: этап «подбор объёма» → «котировка A bsc_dex (зависла)»"
    );
}

#[tokio::test]
async fn no_scope_no_timing() {
    // Вне расчёта кандидата (предпроверка сделки) этапы просто выполняются, запросы не считаются.
    let v = stage(|| "бюджет".into(), async { 5 }).await;
    assert_eq!(v, 5);
    calc_meter::record(Source::Node, 1, Duration::ZERO, Duration::ZERO);
}
