.PHONY: check sync sync-spec locks rust

sync:
	sfw cargo fetch --locked
	python3 scripts/sync_spec.py

sync-spec:
	python3 scripts/sync_spec.py

locks:
	python3 scripts/check_action_pins.py
	python3 scripts/check_contract_locks.py
	python3 scripts/sync_spec.py --check

rust:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets --all-features --locked --offline -- -D warnings
	cargo test --workspace --all-targets --locked --offline

check: locks rust
