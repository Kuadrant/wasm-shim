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

# builds the module and move to deploy folder
build: export BUILD?=debug
build:
	@echo "Building the wasm filter"
    ifeq ($(BUILD), release)
		cargo build --target=wasm32-unknown-unknown --release
    else
		cargo build --target=wasm32-unknown-unknown
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
	find . -type f ! -name '*.proto' -delete && \
	find ./data-plane-api -mindepth 1 ! -regex '^./data-plane-api/envoy\(/.*\)?' -delete && \
	find ./googleapis -mindepth 1 ! -regex '^./googleapis/google\(/.*\)?' -delete && \
	mkdir -p googleapis/google/protobuf/ && \
	cd protobuf/src/google/protobuf/ && \
	mv timestamp.proto descriptor.proto duration.proto wrappers.proto any.proto struct.proto ../../../../googleapis/google/protobuf/
	rm -rf vendor-protobufs/protobuf
	cd vendor-protobufs/data-plane-api/envoy && rm -rf admin watchdog api data && \
	cd service && find . -maxdepth 1 ! -name auth ! -name ratelimit ! -name '.' -exec rm -rf {} \;
	cd vendor-protobufs/googleapis/google && find . -maxdepth 1 ! -name protobuf ! -name rpc ! -name '.' -exec rm -rf {} \;
	cd vendor-protobufs/protoc-gen-validate/ && find . -maxdepth 1 ! -name validate ! -name '.' -exec rm -rf {} \;
	cd vendor-protobufs/udpa/ && find . -maxdepth 1 ! -name udpa ! -name xds ! -name '.' -exec rm -rf {} \;
