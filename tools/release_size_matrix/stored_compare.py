"""Stage the optional saved-CEX-window comparator without altering trading code."""

import json
from pathlib import Path
import re
import sys

from adapt import OPTIONAL_PARAMS, SHARED_PARAMS


TOOLS = Path(__file__).resolve().parent


def stage(target, case_path):
    template = (TOOLS / "stored_compare.rs").read_text(encoding="utf-8-sig")
    ctx = (target / "src/core/uncapped_recompute_ctx.rs").read_text(encoding="utf-8-sig")
    size = (target / "src/core/size.rs").read_text(encoding="utf-8-sig")
    parsed_ctx = re.search(r"(?ms)^    let cx = (DirCtx \{.*?^    \});", ctx)
    parsed_params = re.search(r"(?ms)^pub struct Params \{(.*?)^\}", size)
    if not parsed_ctx or not parsed_params:
        return {"status": "unavailable", "reason": "unrecognized native context or search parameters"}
    names = re.findall(r"(?m)^    pub (\w+):", parsed_params[1])
    replacements = {
        "step": "step_of(plan, book)",
        "budget": "budget",
        "guess": "budget / book.asks[0].price",
        "buy_breaks": "cumulative(&book.asks)",
        "sell_breaks": "cumulative(&sell.bids)",
        "max_evals": "max_evals",
        "reuse": "false",
    }
    fields = SHARED_PARAMS | OPTIONAL_PARAMS | replacements
    if set(names) - fields.keys() or SHARED_PARAMS.keys() - set(names):
        return {"status": "unavailable", "reason": "unrecognized native search parameter fields"}
    ctx_literal = parsed_ctx[1].replace(
        "super::reserve::", "arb::core::reserve::",
    )
    if "bsc_contract: None," in ctx_literal:
        normalize = "old"
    elif "contract: None," in ctx_literal:
        normalize = "new"
    else:
        return {"status": "unavailable", "reason": "unrecognized native asset fields"}
    setup = (
        """let extra = arb::core::size_extra::Extra {
        min_usd: Some(floor.0),
        min_pct: Some(floor.1),
        ..Default::default()
    };"""
        if "ext" in names else "let _ = floor;"
    )
    params_code = "\n".join(f"        {field}: {fields[field]}," for field in names)
    template = template.replace("// NATIVE_DIR_CTX", ctx_literal)
    template = template.replace("// NATIVE_EXTRA", setup).replace("// NATIVE_PARAMS", params_code)
    template = template.replace("NATIVE_ASSET_SCHEMA", normalize)
    template = template.replace("NATIVE_THRESHOLD_PLACEMENT", "inside_search" if "ext" in names else "after_search")

    source = json.loads(case_path.read_text(encoding="utf-8-sig"))["cases"]
    cases = [case for case in source if case["basis"] == "cex_window_books"]
    if not cases:
        return {"status": "unavailable", "reason": "no pairs with both saved CEX ladders"}
    tests = target / "tests"
    (tests / "core_size_stored_compare.rs").write_text(template, encoding="utf-8")
    (tests / "size_stored_cases.json").write_text(json.dumps(cases), encoding="utf-8")
    return {"status": "staged", "cases": len(cases), "expected_rows": len(cases) * 2}


if __name__ == "__main__":
    print(json.dumps(stage(Path(sys.argv[1]), Path(sys.argv[2]))))
