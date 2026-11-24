from pathlib import Path
import re
import shutil
import sys

SHARED_PARAMS = {
    "step": "s.step",
    "budget": "s.budget",
    "guess": "guess",
    "buy_breaks": "s.buy.breaks()",
    "sell_breaks": "s.sell.breaks()",
    "bisect_steps": "12",
    "grid_points": "16",
    "refine_steps": "30",
    "tolerance_pct": "Decimal::ONE",
    "max_evals": "max_evals",
}

OPTIONAL_PARAMS = {
    "par_points": "4",
    "bisect_probes": "3",
    "early_floor": "None",
    "reuse": "true",
    "par_blocked": "false",
    "ext": "extra",
}

PCT_HOOKS = """
    fn pct(data: &Self::Data) -> Option<Decimal> {
        Some(data.0)
    }

    fn pct_base(data: &Self::Data) -> Option<Decimal> {
        Some(data.1)
    }
"""


def adapt(source, target):
    """Overlay a standalone test, independently of the historical test suite."""
    size = (target / "src/core/size.rs").read_text(encoding="utf-8")
    params = re.search(r"(?ms)^pub struct Params \{(.*?)^\}", size)[1]
    names = re.findall(r"(?m)^    pub (\w+):", params)
    unsupported = (set(names) - SHARED_PARAMS.keys() - OPTIONAL_PARAMS.keys()
                   ) | (SHARED_PARAMS.keys() - set(names))
    if unsupported:
        raise SystemExit(f"Unsupported native Params fields: {sorted(unsupported)}")

    native = "ext" in names
    template = (source / "replay.rs").read_text(encoding="utf-8-sig")
    fixture = target / "tests/core_size_release_matrix.rs"
    fixture.write_text(
        template.replace("    // MATRIX_CURVE_HOOKS", PCT_HOOKS if native else ""),
        encoding="utf-8",
    )
    shutil.copyfile(source / "cake.json", target / "tests/size_matrix_cake.json")
    shutil.copyfile(source / "routes.rs", target / "tests/size_matrix_routes.inc")
    fields = SHARED_PARAMS | OPTIONAL_PARAMS
    assignment = "".join(f"        {name}: {fields[name]},\n" for name in names)
    threshold = "inside_search" if native else "after_search"
    setup = "" if not native else """
    let extra = arb::core::size_extra::Extra {
        min_usd: Some(floor.0),
        min_pct: Some(floor.1),
        ..Default::default()
    };
"""
    over_budget = "OverBudget" if re.search(r"(?m)^    OverBudget\(", size) else "TooLarge"
    (target / "tests/size_matrix_adapter.inc").write_text(
        f"""const THRESHOLD_PLACEMENT: &str = "{threshold}";

fn over_budget() -> Fail {{
    Fail::{over_budget}("budget while repricing".into())
}}

fn native_params(
    s: &Scenario,
    guess: Decimal,
    {"floor" if native else "_floor"}: (Decimal, Decimal),
    max_evals: u32,
) -> Params {{
{setup}
    Params {{
{assignment}    }}
}}
""",
        encoding="utf-8",
    )


if __name__ == "__main__":
    adapt(Path(__file__).resolve().parent, Path(sys.argv[1]))
