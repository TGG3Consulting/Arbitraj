# Historical cash-flow control

`project.py` reads two owner's local exports. It removes asset names, addresses, order and deal identifiers, deposits, API details, and the text of all events. Only the financial inputs used by the close and return-cost functions and the time-bucket membership survive. The receipt-verified repair is a separate scenario. It does not modify the ledger or convert any theoretical quote to a fill.

The diagnostic workflow compiles the checked-out `target-sha.txt` on GitHub Actions. Its ephemeral receiver key encrypts the anonymous projection in transit. Only the public key and aggregate result are intentionally exposed by the workflow. The private key and input are removed from the runner at the end. The trading source is used unchanged, with the diagnostic test copied into that checkout.
