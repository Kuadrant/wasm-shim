SHELL := /bin/bash

MKFILE_PATH := $(abspath $(lastword $(MAKEFILE_LIST)))
PROJECT_PATH := $(patsubst %/,%,$(dir $(MKFILE_PATH)))

BUILD ?= debug
ifneq ($(FEATURES),)
	FEATURE_CMD=--features $(FEATURES)
endif

WASM_RELEASE_BIN = $(PROJECT_PATH)/target/wasm32-unknown-unknown/$(BUILD)/wasm_shim.wasm
WASM_RELEASE_PATH = $(dir $(WASM_RELEASE_BIN))

PROTOC_BIN=$(PROJECT_PATH)/bin/protoc
PROTOC_VERSION=21.1
UNAME_S := $(shell uname -s)
ifeq ($(UNAME_S),Linux)
	PROTOC_OS="linux-x86_64"
endif
ifeq ($(UNAME_S),Darwin)
	PROTOC_OS="osx-universal_binary"
endif
$(PROTOC_BIN):
	$(call get-protoc,$(PROJECT_PATH),https://github.com/protocolbuffers/protobuf/releases/download/v${PROTOC_VERSION}/protoc-$(PROTOC_VERSION)-$(PROTOC_OS).zip)

# builds the module and move to deploy folder
build: $(PROTOC_BIN)
	@echo "Building the wasm filter"
    ifeq ($(BUILD), release)
		export PATH=$(PROJECT_PATH)/bin:$$PATH; cargo build --target=wasm32-unknown-unknown --release $(FEATURE_CMD)
    else
		export PATH=$(PROJECT_PATH)/bin:$$PATH; cargo build --target=wasm32-unknown-unknown $(FEATURE_CMD)
    endif

# Remove old ones and fetch the latest third-party protobufs
update-protobufs:
	rm -rf vendor-protobufs/* || true
	cd vendor-protobufs && \
	git clone https://github.com/envoyproxy/data-plane-api.git && \
	git clone https://github.com/envoyproxy/protoc-gen-validate.git && \
	git clone https://github.com/googleapis/googleapis.git && \
	git clone https://github.com/cncf/udpa.git && \
	git clone https://github.com/protocolbuffers/protobuf.git && \
	git clone https://github.com/cncf/xds.git && \
	find . -type f ! -name '*.proto' -delete && \
	find ./data-plane-api -mindepth 1 ! -regex '^./data-plane-api/envoy\(/.*\)?' -delete && \
	find ./googleapis -mindepth 1 ! -regex '^./googleapis/google\(/.*\)?' -delete && \
	find ./xds -mindepth 1 ! -regex '^./xds/xds\(/.*\)?' -delete && \
	mkdir -p googleapis/google/protobuf/ && \
	cd protobuf/src/google/protobuf/ && \
	mv timestamp.proto descriptor.proto duration.proto wrappers.proto any.proto struct.proto empty.proto ../../../../googleapis/google/protobuf/
	rm -rf vendor-protobufs/protobuf
	cd vendor-protobufs/data-plane-api/envoy && rm -rf admin watchdog api data && \
	cd service && find . -maxdepth 1 ! -name auth ! -name ratelimit ! -name '.' -exec rm -rf {} \;
	cd vendor-protobufs/googleapis/google && find . -maxdepth 1 ! -name protobuf ! -name rpc ! -name '.' -exec rm -rf {} \;
	cd vendor-protobufs/protoc-gen-validate/ && find . -maxdepth 1 ! -name validate ! -name '.' -exec rm -rf {} \;
	cd vendor-protobufs/udpa/ && find . -maxdepth 1 ! -name udpa ! -name xds ! -name '.' -exec rm -rf {} \;
# Rust protobuf support doesn't allow multiple files with same name and diff dir to merge so we have to create one our own.
#	cd vendor-protobufs/data-plane-api/envoy/type/ && \
#	touch tmp && git merge-file ./matcher/v3/metadata.proto ./tmp ./metadata/v3/metadata.proto --own && rm tmp

RUST_SOURCES := $(shell find $(PROJECT_PATH)/src -name '*.rs')

$(WASM_RELEASE_BIN): $(RUST_SOURCES)
	make -C $(PROJECT_PATH) -f $(MKFILE_PATH) build BUILD=$(BUILD)

# get-protoc will download zip from $2 and install it to $1.
define get-protoc
@{ \
echo "Downloading $(2) and installing in $(1)/bin" ;\
set -e ;\
TMP_DIR=$$(mktemp -d) ;\
cd $$TMP_DIR ;\
curl -Lo protoc.zip $(2) ;\
unzip -q protoc.zip bin/protoc -d $(1)/. ;\
chmod a+x $(1)/bin/protoc ;\
rm -rf $$TMP_DIR ;\
}
endef

##@ Util

# go-install-tool will 'go install' any package $2 and install it to $1.
PROJECT_DIR := $(shell dirname $(abspath $(lastword $(MAKEFILE_LIST))))
define go-install-tool
@[ -f $(1) ] || { \
set -e ;\
TMP_DIR=$$(mktemp -d) ;\
cd $$TMP_DIR ;\
go mod init tmp ;\
echo "Downloading $(2)" ;\
GOBIN=$(PROJECT_DIR)/bin go install $(2) ;\
rm -rf $$TMP_DIR ;\
}
endef

include ./make/*.mk
