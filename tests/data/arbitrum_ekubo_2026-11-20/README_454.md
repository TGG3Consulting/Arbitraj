# Ekubo (Arbitrum One) — дополнительная запись (решение 454)

tools/arbitrum_record/ekubo_more.py: только чтение. Блок B = 526015513 (0x1f5a5c19), время 1795201090.
PoolInitialized с блока 416469411 (квитанция развёртывания Core в contracts/evmtest/vendor/ekubo/broadcast/DeployAll.s.sol/42161/run-1767194711580.json) до B: {"total": 22, "native": 8, "extensions": {"0x0000000000000000000000000000000000000000": 12, "0xd4f1060cb9c1a13e1d2d20379b8aa2cf7541ed9b": 1, "0x406a6e4f256dd43bd8835ce84d108e1e4476d5e4": 1, "0x409bd88ab297f351d20f576893f989d0b8f85049": 1, "0xd47f1b1edcfeabb08f6ebd8fc337c27e636c75ba": 2, "0x611953b8b2ecfe5692e189b3ce4e647c6e83455e": 2, "0x589dffa001c81b691b4e79a0048625f9b80b726a": 1, "0x659f0793f6c9d4eec32967df986c52e8b0a4629f": 1, "0x6ae4d63bcd054c5698cac06a7bc5754632ee267e": 1}, "not_in_key_index": 22}.
Решение 460: поиск PoolInitialized — до чтения состояния; блок B взят после поиска (узел хранит только недавнее состояние), хвост логов до B дочитан.

## SHA-256

8ea7c4a1f027f0e94e14c21ed841f3b3227b023c6bb9695379f06f0765375260  more_meta.json
b97df800be7180f291a591fd7d93dcefdcf37c7fa22983ff12baf14521d93ec8  pool_initialized.json
97fc1146a92408838a3be4a5cb9bdfef4c3bb24b1aa8077196f4781712c6897c  native_pools.json
25add2c0beb23b2f51b592ef3c78bbf6df493d46d0b44adfb4b898221a6bffc5  twamm_pools.json
3dd2d647494b202c32efebfed026fba886666dbe6d115ba77e57dbd3e45d4af5  core_load.json
