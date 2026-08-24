# `base-shadow-indexer`

Shadow indexer Execution Extension (`ExEx`) that captures reorged-out and reverted execution
blocks and persists their metadata to the shadow indexer database. Canonical blocks are not
persisted: only blocks the chain discarded carry shadow-block signal.

A `ChainReorged` names the canonical replacement only for heights its `new` chain covers. The
shadow builder swaps its speculative chain one Engine API round trip at a time, so `new` is
routinely a single block against five displaced ones, and the rest of the replacements arrive
as later `ChainCommitted` notifications. Those commits are forwarded to the writer purely to
fill in `canonical_hash` on the rows already stored at those heights.
