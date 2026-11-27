"""Selectors of single-maker RFQ actions and the official Settler registry used by both networks."""
import re

REGISTRY='0x00000000000004533fe15556b1e086bb1a72ceae'
PATTERN=re.compile(r'd92aadfb[0-9a-f]{64}000000000000000000000000([0-9a-f]{40})([0-9a-f]{64})|(?:604ba49a|9714f25e)[0-9a-f]{344}([0-9a-f]{40})([0-9a-f]{64})')
