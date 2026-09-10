
##@ Verify

RATCHET = $(PROJECT_PATH)/bin/ratchet
RATCHET_VERSION = v0.11.4
$(RATCHET):
	$(call go-install-tool,$(RATCHET),github.com/sethvargo/ratchet@$(RATCHET_VERSION))

.PHONY: ratchet
ratchet: $(RATCHET) ## Download ratchet locally if necessary.

.PHONY: ratchet-pin
ratchet-pin: ratchet ## Pin GitHub Actions to commit SHAs.
	$(RATCHET) pin $$(find .github/workflows .github/actions -name '*.yaml' -o -name '*.yml')

.PHONY: verify-ratchet
verify-ratchet: ratchet ## Verify GitHub Actions are pinned to commit SHAs.
	$(RATCHET) lint $$(find .github/workflows .github/actions -name '*.yaml' -o -name '*.yml')
