##@ Kind

.PHONY: kind kind-create-cluster kind-delete-cluster

NAMESPACE ?= kuadrant-system
KIND = $(PROJECT_PATH)/bin/kind
KIND_VERSION = v0.23.0
$(KIND):
	$(call go-install-tool,$(KIND),sigs.k8s.io/kind@$(KIND_VERSION))

kind: $(KIND) ## Download kind locally if necessary.

KIND_CLUSTER_NAME ?= wasm-auth-local

kind-create-cluster: WASM_PATH=$(subst /,\/,$(WASM_RELEASE_PATH))
kind-create-cluster: kind ## Create the "wasm-auth-local" kind cluster.
	@{ \
  	TEMP_FILE=/tmp/kind-cluster-$$(openssl rand -hex 4).yaml ;\
  	cp $(PROJECT_PATH)/utils/kind/cluster.yaml $$TEMP_FILE ;\
	$(SED) -i "s/\$$(WASM_PATH)/$(WASM_PATH)/g" $$TEMP_FILE ;\
	KIND_EXPERIMENTAL_PROVIDER=$(CONTAINER_ENGINE) $(KIND) create cluster --name $(KIND_CLUSTER_NAME) --config $$TEMP_FILE ;\
	rm -rf $$TEMP_FILE ;\
	}

kind-delete-cluster: ## Delete the "wasm-auth-local" kind cluster.
	- KIND_EXPERIMENTAL_PROVIDER=$(CONTAINER_ENGINE) $(KIND) delete cluster --name $(KIND_CLUSTER_NAME)

KUSTOMIZE = $(PROJECT_PATH)/bin/kustomize
$(KUSTOMIZE):
	$(call go-install-tool,$(KUSTOMIZE),sigs.k8s.io/kustomize/kustomize/v4@v4.5.5)

.PHONY: kustomize
kustomize: $(KUSTOMIZE) ## Download kustomize locally if necessary.

##@ Authorino

.PHONY: namespace
namespace: ## Creates a namespace $(NAMESPACE)
	kubectl create namespace $(NAMESPACE)

.PHONY: install-authorino-operator
install-authorino-operator: $(KUSTOMIZE) ## Installs Authorino Operator and dependencies into the Kubernetes cluster configured in ~/.kube/config
	$(KUSTOMIZE) build $(PROJECT_PATH)/utils/kustomize/authorino-operator | kubectl apply -f -
	kubectl -n "$(NAMESPACE)" wait --timeout=300s --for=condition=Available deployments --all

.PHONY: deploy-authorino
deploy-authorino: $(KUSTOMIZE) ## Deploys an instance of Authorino into the Kubernetes cluster configured in ~/.kube/config
	$(KUSTOMIZE) build $(PROJECT_PATH)/utils/kustomize/authorino | kubectl apply -f -
	kubectl -n "$(NAMESPACE)" wait --timeout=300s --for=condition=Available deployments --all

##@ Limitador

.PHONY: install-limitador-operator
install-limitador-operator: $(KUSTOMIZE) ## Installs Limitador Operator and dependencies into the Kubernetes cluster configured in ~/.kube/config
	$(KUSTOMIZE) build $(PROJECT_PATH)/utils/kustomize/limitador-operator | kubectl apply -f -
	kubectl -n "$(NAMESPACE)" wait --timeout=300s --for=condition=Available deployments --all

.PHONY: deploy-limitador
deploy-limitador:
	$(KUSTOMIZE) build $(PROJECT_PATH)/utils/kustomize/limitador | kubectl apply -f -

##@ User Apps

.PHONY: user-apps

user-apps: ## Deploys talker API and envoy
	kubectl -n $(NAMESPACE) apply -f https://raw.githubusercontent.com/kuadrant/authorino-examples/main/talker-api/talker-api-deploy.yaml
	kubectl -n $(NAMESPACE) apply -f $(PROJECT_PATH)/utils/deploy/envoy.yaml
	kubectl -n $(NAMESPACE) apply -f $(PROJECT_PATH)/utils/deploy/authconfig.yaml

##@ Util

.PHONY: local-setup local-env-setup local-cleanup local-rollout sed

local-setup: local-env-setup
	kubectl -n $(NAMESPACE) wait --timeout=300s --for=condition=Available deployments --all
	@{ \
	echo "Now you can export the envoy service by doing:"; \
	echo "kubectl port-forward --namespace $(NAMESPACE) deployment/envoy 8000:8000"; \
	echo "After that, you can curl -H \"Host: myhost.com\" localhost:8000"; \
	}

local-env-setup: $(WASM_RELEASE_BIN)
	$(MAKE) kind-delete-cluster
	$(MAKE) kind-create-cluster
	$(MAKE) namespace
	$(MAKE) install-authorino-operator
	$(MAKE) install-limitador-operator
	$(MAKE) deploy-authorino
	$(MAKE) deploy-limitador
	$(MAKE) user-apps

local-cleanup: kind ## Delete the "wasm-auth-local" kind cluster.
	$(MAKE) kind-delete-cluster

local-rollout:
	$(MAKE) user-apps
	kubectl rollout restart -n $(NAMESPACE) deployment/envoy
	kubectl -n $(NAMESPACE) wait --timeout=300s --for=condition=Available deployments --all

ifeq ($(shell uname),Darwin)
SED=$(shell which gsed)
else
SED=$(shell which sed)
endif
sed: ## Checks if GNU sed is installed
ifeq ($(SED),)
	@echo "Cannot find GNU sed installed."
	exit 1
endif
