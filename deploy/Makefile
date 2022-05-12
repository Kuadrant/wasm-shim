MKFILE_PATH := $(abspath $(lastword $(MAKEFILE_LIST)))
PROJECT_PATH := $(patsubst %/,%,$(dir $(MKFILE_PATH)))
IMAGE_VERSION ?= latest
DOCKER ?= podman

.PHONY: build
build:
	$(DOCKER) build -t quay.io/rahanand/wasm-shim:$(IMAGE_VERSION) -f $(PROJECT_PATH)/Dockerfile

.PHONY: clean
clean:
	$(DOCKER) rmi quay.io/rahanand/wasm-shim:$(IMAGE_VERSION)

.PHONY: push
push:
	$(DOCKER) push quay.io/rahanand/wasm-shim:$(IMAGE_VERSION)