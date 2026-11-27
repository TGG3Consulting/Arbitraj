"""Archived Ethereum market improvements; the registry listing and the paying vault have different roles.

Witnesses for these tuples are captured in record_prop_cash_receipts.py. The fixed terminal
hash is the shared Base/Ethereum integration observation on Ethereum.
"""
REFERENCE = {26494988:"0x2372196a2961b307333be48d3dad74a2e723fd459c2cb01e225a0a97f50e7a7a"}
CASES = {
    "kaliber": (
        "0xb8d4e06e3038b07b4376ba3020eabce69f673463","0xf639cf213b63f7e77d699ff686d591c0ba55fc63",
        {26492541:"0x0a1198c72e4a93ecfa1aa8b64a9cd916f4687bb1cb11b913d543fcf26ae78320", **REFERENCE}
    ),
    "taurus": (
        "0x20861433d9119b866eeb398cb1ad46652a3cf953","0x8b2833425d751c1fbfb5c6d43d209ff9b366439e",
        {26492615:"0x7f5b711d2d4511dcab3a76b41f9d8cdc186c956becc1ec8435515be0e443da94",
         26492655:"0x81249c1dd6e780c609401a7b1a473405668a648751cdefdf69fd4d44ad0f9588",
         26494725:"0x9314a1ee639ec31bc75c6321f109fe0d2bdc18b332ee7e85d0d74b9125a3b4a8", **REFERENCE}
    )
}
