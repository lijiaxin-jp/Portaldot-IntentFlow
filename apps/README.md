# Portaldot operator dashboard

The former dashboard and API informed the views for runs, pool state, execution
evidence, and reproducibility reports. Their implementation is tightly coupled
to the frozen Base replay schema and is not copied as if it were chain neutral.

The dashboard is a local, read-only view over the SQLite evidence database. It
does not sign, submit, or send transactions to the chain. Runtime and pool
details come from the latest persisted snapshots, so an empty database is
shown as uncollected instead of being filled with sample values.

The interface exposes:

- persisted runtime and pool snapshots;
- intent, quote, risk breakdown, and route commitment;
- dry-run and transaction evidence;
- protection failures and replayable input hashes.

Run it from the repository root with `npm run p7:dashboard`. It binds to
`127.0.0.1:8787` by default. Set `INTENTFLOW_EVIDENCE_DB` to use another SQLite
file, or `PORT` to choose another port.

The public acceptance bundle is generated with `npm run p8:acceptance`.
Generation reads the same database and writes to
`artifacts/p8-public-acceptance`; it reports missing deployments, fewer than
ten finalized transactions, and insufficient performance samples as gaps.

This boundary preserves the old platform's useful operator workflow without
carrying over Base addresses, DEX names, replay tables, or broadcast controls.
