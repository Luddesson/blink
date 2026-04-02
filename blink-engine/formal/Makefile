# Formal Verification with Halmos (symbolic execution for Solidity)
#
# Prerequisites:
#   pip install halmos
#   curl -L https://foundry.paradigm.xyz | bash && foundryup
#
# Usage:
#   make verify          — run all Halmos symbolic proofs
#   make verify-signer   — verify EIP-712 signer properties only
#   make verify-risk     — verify risk manager properties only

HALMOS := halmos
FORGE  := forge

.PHONY: verify verify-signer verify-risk build

build:
	$(FORGE) build

verify: verify-signer verify-risk

verify-signer: build
	$(HALMOS) --contract OrderSignerProperties --function check_ --solver-timeout-assertion 30000

verify-risk: build
	$(HALMOS) --contract RiskManagerProperties --function check_ --solver-timeout-assertion 30000
