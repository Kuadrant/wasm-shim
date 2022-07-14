MKFILE_PATH := $(abspath $(lastword $(MAKEFILE_LIST)))
PROJECT_PATH := $(patsubst %/,%,$(dir $(MKFILE_PATH)))
IMAGE_VERSION ?= latest
DOCKER ?= podman
IMAGE_REPOSITORY ?= rahanand

.PHONY: build
build-image:
	$(DOCKER) build -t quay.io/$(IMAGE_REPOSITORY)/wasm-shim:$(IMAGE_VERSION) -f $(PROJECT_PATH)/Dockerfile

.PHONY: clean
clean-image:
	$(DOCKER) rmi quay.io/$(IMAGE_REPOSITORY)/wasm-shim:$(IMAGE_VERSION)

.PHONY: push
push-image:
	$(DOCKER) push quay.io/$(IMAGE_REPOSITORY)/wasm-shim:$(IMAGE_VERSION)