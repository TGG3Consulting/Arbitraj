"""Install the optional native historical-pool routing test into a release checkout."""

import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys


HARNESS_TOOLS = Path(__file__).resolve().parent
FIXTURES = (
    "routes_2026-10-29",
    "routes_2026-10-30",
    "fit_volume_2026-11-15",
)
INPUT_FIELDS = {
    "asset", "side", "qty", "coin", "usdt", "taxes", "coin_dec",
    "usdt_dec", "entries", "mids", "gas", "usdt_ticker", "smart",
}


def install(target):
    route_path = target / "src/venues/bsc_dex/quote/routing.rs"
    helper = target / "tests/route_common/mod.rs"
    test_root = target / "tests"
    sha = subprocess.check_output(
        ["git", "-C", str(target), "rev-parse", "HEAD"], text=True,
    ).strip()
    meta = {"sha": sha, "status": "unavailable", "fixtures": {}}
    if not route_path.is_file() or not helper.is_file():
        return meta | {"reason": "Native routing entry point or route_common missing"}

    route = route_path.read_text(encoding="utf-8-sig")
    routing_input = re.search(r"(?ms)^pub struct Input \{(.*?)^\}", route)
    entry = "pub async fn run(q: Quoting, i: Input) -> GatewayResult<Quote>"
    if (not routing_input or entry not in route
            or set(re.findall(r"(?m)^    pub (\w+):", routing_input[1])) != INPUT_FIELDS):
        return meta | {"reason": "Native routing::Input or routing::run signature differs"}

    # Keep each checkout's constructors for its native pool structures, while
    # injecting identical recorded responses and explicit scenario settings.
    original = helper.read_text(encoding="utf-8-sig")
    old_asset = "bsc_contract: Some(" in original
    if not old_asset and not ("chain: None," in original and "contract: Some(" in original):
        return meta | {"reason": "Native helper uses an unknown asset representation"}
    helper_text = original
    for name in ("load", "v2", "v3", "ctx"):
        helper_text, count = re.subn(
            rf"(?m)^(?:pub )?((?:async )?fn {name}\()", r"pub \1", helper_text,
        )
        if count != 1:
            return meta | {"reason": f"Native helper has no unique {name} constructor"}

    source = HARNESS_TOOLS.parents[1]
    fit = source / "tests/fit_common/mod.rs"
    fixture_files = [source / "tests/data" / f / "responses.json" for f in FIXTURES]
    if not fit.is_file() or not all(f.is_file() for f in fixture_files):
        return meta | {"reason": "Harness is missing its archived pool responses or SHOPINX loader"}

    fit_text = fit.read_text(encoding="utf-8-sig")
    template = (HARNESS_TOOLS / "native_routes.rs").read_text(encoding="utf-8-sig")
    if old_asset:
        for key, value in {"fit_text": fit_text, "template": template}.items():
            patched = re.sub(r"(?m)^\s*chain: None,\n", "", value)
            patched = patched.replace("contract: Some(", "bsc_contract: Some(")
            if key == "fit_text":
                fit_text = patched
            else:
                template = patched

    copied_helper = test_root / "native_routes_common"
    copied_helper.mkdir(exist_ok=True)
    for file in helper.parent.glob("*.rs"):
        if file.name != "mod.rs":
            shutil.copyfile(file, copied_helper / file.name)
    (copied_helper / "mod.rs").write_text(helper_text, encoding="utf-8")
    (test_root / "native_routes_shopinx.rs").write_text(fit_text, encoding="utf-8")
    (test_root / "native_route_release_matrix.rs").write_text(template, encoding="utf-8")
    for fixture, file in zip(FIXTURES, fixture_files, strict=True):
        dest = test_root / "data" / fixture / "responses.json"
        dest.parent.mkdir(parents=True, exist_ok=True)
        contents = file.read_bytes()
        dest.write_bytes(contents)
        meta["fixtures"][fixture] = hashlib.sha256(contents).hexdigest()
    return meta | {"status": "staged", "expected_rows": 80}


if __name__ == "__main__":
    print(json.dumps(install(Path(sys.argv[1]).resolve()), sort_keys=True))
