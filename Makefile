SHELL := /bin/bash

MKFILE_PATH := $(abspath $(lastword $(MAKEFILE_LIST)))
PROJECT_PATH := $(patsubst %/,%,$(dir $(MKFILE_PATH)))

KIND_VERSION=v0.11.1
# Installs kind if not present.
kind:
ifneq ($(KIND_VERSION), $(shell kind version | cut -d' ' -f2))
	go install sigs.k8s.io/kind@$(KIND_VERSION)
KIND=$(GOBIN)/kind
else
KIND=$(shell which kind)
endif

KIND_CLUSTER_NAME ?= wasm-shim-cluster

WASM_RELEASE_PATH = $(PROJECT_PATH)/target/wasm32-unknown-unknown/release/wasm_shim.wasm

# Start a local Kubernetes cluster using Kind
.PHONY: cluster-up
cluster-up: kind
	kind create cluster --name $(KIND_CLUSTER_NAME)

# Intialize submodule
submodule:
	git submodule update --init

# Install Authorino CRDs into a cluster
install-authorino: submodule
	cd authorino-operator && \
	make install

# Uninstall Authorino CRDs from a cluster
uninstall-authorino: submodule
	cd authorino-operator && \
	make uninstall

# Deploys authorino
deploy-authorino:
	cd authorino-operator && make deploy
	kubectl apply -f deploy/authorino.yaml

# Undeploys authorino
undeploy-authoriono:
	cd authorino-operator && make undeploy
	kubectl delete -f deploy/authorino.yaml

# Install Limitador CRDs into a cluster
install-limitador: submodule
	cd limitador-operator && \
	make install

# Uninstall Limitador CRDs from a cluster
uninstall-limitador: submodule
	cd limitador-operator && \
	make uninstall

# Deploys authorino
deploy-limitador:
	cd limitador-operator && make deploy
	kubectl apply -f deploy/limitador.yaml

# Undeploys authorino
undeploy-limitador:
	cd limitador-operator && make undeploy
	kubectl delete -f deploy/limitador.yaml

# Deploys the example
example-up:
	istioctl kube-inject -f deploy/toystore.yaml | kubectl apply -f -
	kubectl apply -f deploy/wasmplugin.yaml
#   Cluster wide resource
	kubectl apply -f deploy/gateway-class.yaml
#   Istio-system resource
	kubectl apply -f deploy/gateway.yaml
	kubectl apply -f deploy/http-route.yaml
	kubectl apply -f deploy/authorino-simple-api.yaml

# Clean up the example
example-down:
	istioctl kube-inject -f deploy/toystore.yaml | kubectl delete -f -
	kubectl delete -f deploy/wasmplugin.yaml
	kubectl delete -f deploy/gateway-class.yaml
	kubectl delete -f deploy/gateway.yaml
	kubectl delete -f deploy/http-route.yaml
	kubectl delete -f deploy/authorino-simple-api.yaml


PROTOC_BIN=$(PROJECT_PATH)/bin/protoc
PROTOC_VERSION=21.1
$(PROTOC_BIN):
	mkdir -p $(PROJECT_PATH)/bin
	$(call get-protoc,$(PROJECT_PATH)/bin,https://github.com/protocolbuffers/protobuf/releases/latest/download/protoc-$(PROTOC_VERSION)-linux-x86_64.zip,sigs.k8s.io/controller-tools/cmd/controller-gen@v0.3.0)

# builds the module and move to deploy folder
build: export BUILD?=debug
build: $(PROTOC_BIN)
	@echo "Building the wasm filter"
    ifeq ($(BUILD), release)
		export PATH=$(PROJECT_PATH)/bin:$$PATH; cargo build --target=wasm32-unknown-unknown --release
    else
		export PATH=$(PROJECT_PATH)/bin:$$PATH; cargo build --target=wasm32-unknown-unknown
    endif
	cp target/wasm32-unknown-unknown/$(BUILD)/*.wasm ./deploy/

# Deletes the local Kubernetes cluster started using Kind
.PHONY: cleanup
cleanup: kind
	kind delete cluster --name $(KIND_CLUSTER_NAME)

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

$(WASM_RELEASE_PATH): export BUILD = release
$(WASM_RELEASE_PATH):
	make -C $(PROJECT_PATH) -f $(MKFILE_PATH) build

development: $(WASM_RELEASE_PATH)
	docker-compose up

stop-development:
	docker-compose down

# get-protoc will download zip from $2 and install it to $1.
define get-protoc
@[ -f $(1) ] || { \
echo "Downloading $(2) and installing in $(1)" ;\
set -e ;\
TMP_DIR=$$(mktemp -d) ;\
cd $$TMP_DIR ;\
curl -Lo protoc.zip $(2) ;\
unzip -q protoc.zip bin/protoc ;\
cp bin/protoc $(1) ;\
chmod a+x $(1)/protoc ;\
rm -rf $$TMP_DIR ;\
}
endef
