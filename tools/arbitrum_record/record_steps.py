"""Файл: tools/arbitrum_record/record_steps.py
Что делает: решение 459 (T456-03, T457-02) — список скриптов записи узла Arbitrum для record_all.py: имя, файл и
  аргументы запуска. Адреса — те же официальные значения, что в шапке каждого скрипта и в README этой папки
  (источник — там же: docs/notes/arbitrum/dex-settings-arbitrum.md, docs/FIRST_RUN.md, официальные репозитории).
  Порядок важен: balancer_v3.py до exec_v4_live.py (пулы Balancer V3 приёмка берёт из его записи), ekubo.py до
  ekubo_more.py (та же папка записи).
Кто вызывает: tools/arbitrum_record/record_all.py.
От чего зависит: нет.
Ошибок нет.
"""

# Камелот V3 (решение 444): хеши кода пула — README этой папки, раздел algebra_dir_fee.py.
ALGEBRA_HASHES = [
    "0x77066034e31fe5172b86f4cf68f7e01dd6954f2e8292e6796c9bf7e99dbae477",
    "0xb785aaed76f65b185ee1fe081ef2233b7473b6c47bc008865f64dac83f4982f4",
    "0xb994c319655d0a2474f63ef4c8c5c9ee051c08ddd25e75c6a36c89e8a2a229e7",
    "0x4e92c1fd81a5c8ce82f5a90742e3376c5b258017e149fd0629068ecc9aab43c6",
    "0xac97ff1b66ef7517262cbf16db0195b400714e8ecb499fc86f349e77f50ae89b",
    "0xdcb95b809dca000f4ce04aa004b723fef2dbb3704db3c0389699bc70f0e42c79",
    "0x0ca66607e4d0ece7798fce9ae13cad1a28be1eee443451573f92ba6e3bcd92b9",
    "0x5d100e2dbf4b80b9451c29380def0d6c4abeb3db0d166c81fda155a89e106dbd",
    "0xab9acbcae566bd662fb801895320dc3dd170e64394f78600b969ab128a3a40fd",
    "0x6c1bebd370ba84753516bc1393c0d0a6c645856da55f5393ac8ab3d6dbc861d3",
]

BV3_LISTS = [
    "dex.bv3_weighted_factories=0xD961E30156C2E0D0d925A0De45f931CB7815e970",
    "dex.bv3_weighted_min_factories=0x83bf399FA3DC49Af8fb5c34031a50c7C93F56129",
    "dex.bv3_stable_factories=0xEB1eeaBF0126d813589C3D2CfeFFE410D9aE3863",
    "dex.bv3_stable_open_factories=0x44d33798dddCdAbc93Fe6a40C80588033Dc502d3,0x86e67E115f96DF37239E0479441303De0de7bc2b,"
    "0x201efd508c8DfE9DE1a13c2452863A78CB2a86Cc",
    "dex.bv3_stable_guard_factories=0x1FC7F1F84CFE61a04224AC8D3F87f56214FeC08c,0x8A8B9f35765899B3a0291700141470D79EA2eA88",
    "dex.bv3_gyro_2clp_factories=0x65A22Ec32c37835Ad5E77Eb6f7452Ac59E113a9F,0x64b7dfE8a3972639712EFA124D180DC3Ff0f5bcF",
    "dex.bv3_gyro_eclp_factories=0x88ED12A90142fDBFe2a28f7d5b48927254C7e760,0xe31715e75207acC8bfadd96902FF522058928479",
    "dex.bv3_reclamm_factories=0xa1D0791a41318c775707C56eAe247AF81a05322C,0x355bD33F0033066BB3DE396a6d069be57353AD95",
    "dex.bv3_reclamm_v3_factories=0x3ccD78683efFffdDc1A16f5553C896ac6D3ab7FF",
    "dex.bv3_lbp_v1_factories=0x4BB42f71CAB7Bd13e9f958dA4351B9fa2d3A42FF",
    "dex.bv3_lbp_v2_factories=0x8D217CB74f675B46cC2767C8cF0aAB53BE1c4818",
    "dex.bv3_lbp_v3_factories=0xF9309a99836b5F07a2440c177C049b0f0A9A2c33",
    "dex.bv3_lbp_v4_factories=0x6b7DEd11F15D6AB9959c29cEd3Cd101D69A14c35",
    "dex.bv3_fixed_lbp_factories=0x7f246E7Bab4CdC8C7AB41EaDA8290009a5b26E0D",
    "dex.bv3_quantamm_factories=0x62B9eC6A5BBEBe4F5C5f46C8A8880df857004295",
    "dex.bv3_stable_surge_hooks=0x0Fa0f9990D7969a7aE6f9961d663E4A201Ed6417,0x7c1b7A97BfAcD39975dE53e989A16c7BC4C78275",
    "dex.bv3_mev_capture_hooks=0x5B42eC6D40f7B7965BE5308c70e2603c0281C1E9",
]


def steps(rpc, wallet, ekubo_from):
    """Список (имя, файл, аргументы). Принимает: адрес узла, кошелёк (или None), блок ядра Ekubo (или None — ekubo_more
    пропускается с причиной). Ошибок нет."""
    algebra = ["--rpc", rpc, "--factory", "0x1a3c9B1d2F0529D97f2afC5136Cc23e58f1FD35B",
               "--quoter", "0x0Fc73040b26E9bC8514fA028D998E73A254Fa76E", "--gecko-dex", "camelot-v3",
               "--gecko-pools", "5", "--words", "8", "--bps", "1", "--bps", "10", "--bps", "100",
               "--log-blocks", "20000", "--vq-runtime", "contracts/out/VirtualQuoter.bin-runtime", "--out", "tests/data"]
    for h in ALGEBRA_HASHES:
        algebra += ["--init-hash", h]
    bv3 = ["--rpc", rpc, "--vault", "0xbA1333333333a1BA1108E8412f11850A5C319bA9",
           "--router", "0xEAedc32a51c510d35ebC11088fD5fF2b47aACF2E", "--gt-pages", "5"]
    for item in BV3_LISTS:
        bv3 += ["--list", item]
    # Решение 474: буферы ERC4626 — BatchRouter 20241205-v3-batch-router (output/arbitrum.json), опорная сумма превью —
    # совет docs/FIRST_RUN.md (dex.bv3_buffer_preview_raw).
    bv3buf = ["--rpc", rpc, "--vault", "0xbA1333333333a1BA1108E8412f11850A5C319bA9",
              "--batch-router", "0xaD89051bEd8d96f045E8912aE1672c6C0bF8a85E",
              "--preview-raw", "1000000000000000000000000000000"]
    for item in BV3_LISTS:
        bv3buf += ["--list", item]
    out = [
        ("sub_dex", "sub_dex.py", [
            "--node", rpc, "--blocks", "2000000", "--log-step", "100000", "--max-pools", "20",
            "--kyber", "DMM:0xD9bfE9979e9CA4b2fe84bA5d4Cf963bBcB376974:0xEaE47c5D99f7B31165a7f0c5f7E0D6afA25CFd55",
            "--kyber", "KS:0x1c758aF0688502e49140230F6b0EBd376d429be5:0x5649B4DD00780e99Bab7Abb4A3d581Ea1aEB23D0",
            "--kyber", "KS_LEGACY:0x51E8D106C646cA58Caf32A47812e95887C071a62:0xC3E2aED41ECdFB1ad41ED20D45377Da98D5489dD",
            "--vault", "0xBA12222222228d8Ba445958a75a0704d566BF2C8",
            "--balancer-factory", "w1:v1:0x7dFdEF5f355096603419239CE743BfaF1120312B",
            "--balancer-factory", "w2t:v1:0xCF0a32Bbef8F064969F21f7e02328FB577382018",
            "--balancer-factory", "s1:v1:0x2433477A10FC5d31B9513C638F19eE85CaED53Fd",
            "--balancer-factory", "wv2:v2:0x8df6EfEc5547e31B0eb7d1291B511FF8a2bf987c",
            "--balancer-factory", "sv2:v2:0xEF44D6786b2b4d544b7850Fe67CE6381626Bf2D6",
            "--balancer-factory", "wv3:v2:0xf1665E19bc105BE4EDD3739F88315cC699cc5b65",
            "--balancer-factory", "wv4:v2:0xc7E5ED1054A24Ef31D827E6F86caA58B3Bc168d7",
            "--izi-factory", "0xCFD8A067e1fa03474e79Be646c5f6b6A27847399", "--izi-fee", "100", "--izi-fee", "400",
            "--izi-fee", "2000", "--izi-fee", "10000", "--pancake-ur", "0xFE6508f0015C778Bdcc1fB5465bA5ebE224C9912"]),
        ("kyber_elastic_v1", "kyber_elastic_v1.py", ["--rpc", rpc, "--gt"]),
        ("saddle", "saddle.py", ["--rpc", rpc]),
        ("woofi", "woofi.py", ["--rpc", rpc]),
        ("fluid_t1", "fluid_t1.py", ["--node", rpc, "--factory", "0x91716C4EDA1Fb55e84Bf8b4c7085f84285c19085",
                                     "--resolver", "0x05Bd8269A20C472b148246De20E6852091BF16Ff",
                                     "--wrapped", "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1", "--gt"]),
        ("balancer_v3", "balancer_v3.py", bv3),
        ("balancer_v3_buffer", "balancer_v3_buffer.py", bv3buf),
        ("curve_crypto", "curve_crypto.py", [
            "--node", rpc, "--twocrypto-factory", "0x98EE851a00abeE0d95D08cF4CA2BdCE32aeaAF7F",
            "--twocrypto-math", "0x1Fd8Af16DC4BEBd950521308D55d0543b6cDF4A1",
            "--twocrypto-math-v200", "0x2005995a71243be9FB995DaB4742327dc76564Df",
            "--twocrypto-impls", "0x934791f7F391727db92BFF94cd789c4623d14c52,0x04Fd6beC7D45EFA99a27D29FB94b55c56dD07223",
            "--tricrypto-factory", "0xbC0797015fcFc47d9C1856639CaE50D0e69FbEE8",
            "--tricrypto-math", "0x604388Bb1159AFd21eB5191cE22b4DeCdEE2Ae22",
            "--tricrypto-impls", "0xd7E72f3615aa65b92A4DBdC211E296a35512988B,0x1f7C86AffE5bCF7a1D74a8c8E2ef9E03BF31c1BD",
            "--v1-factory", "0xE5De15A9C9bBedb4F5EC13B131E61245f2983A69",
            "--v1-template", "0x5bcA7dDF1bcccB2eE8e46c56bfc9d3CDC77262bC",
            "--tricrypto-v1-pool", "0x960ea3e3C7FB317332d990873d354E18d7645590"]),
        ("camelot_v2", "camelot_v2.py", [rpc, "--factory", "0x6EcCab422D763aC031210895C81787E87B43A652"]),
        ("algebra_dir_fee", "algebra_dir_fee.py", algebra),
        ("lb_v20", "lb_v20.py", [rpc, "--factory", "0x1886D09C9Ade0c5DB822D85D21678Db67B6c2982",
                                 "--router", "0x7BFd7192E76D950832c77BB412aaE841049D8D9B"]),
        ("lb_v22", "lb_v22.py", [rpc, "--factory", "0xb43120c4745967fa9b93E79C149E66B0f2D6Fe0c"]),
        ("ekubo", "ekubo.py", ["--rpc", rpc, "--gt"]),
        ("ekubo_more", "ekubo_more.py", ["--rpc", rpc, "--from-block", str(ekubo_from)] if ekubo_from is not None
         else None),
        ("exec_v4_live", "exec_v4_live.py", ["--rpc", rpc] + (["--wallet", wallet] if wallet else [])),
    ]
    return out
