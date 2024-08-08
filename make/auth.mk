##@ Kind

.PHONY: kind kind-create-cluster kind-delete-cluster

KIND = $(PROJECT_PATH)/bin/kind
KIND_VERSION = v0.23.0
$(KIND):
	$(call go-install-tool,$(KIND),sigs.k8s.io/kind@$(KIND_VERSION))

kind: $(KIND) ## Download kind locally if necessary.

KIND_CLUSTER_NAME ?= wasm-auth-local

kind-create-cluster: BUILD?=debug
kind-create-cluster: WASM_PATH=$(subst /,\/,$(PROJECT_PATH)/target/wasm32-unknown-unknown/$(BUILD))
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


##@ Authorino

.PHONY: install-authorino-operator certs deploy-authorino

AUTHORINO_IMAGE ?= quay.io/kuadrant/authorino:latest
AUTHORINO_OPERATOR_NAMESPACE ?= authorino-operator
install-authorino-operator: ## Installs Authorino Operator and dependencies into the Kubernetes cluster configured in ~/.kube/config
	curl -sL https://raw.githubusercontent.com/Kuadrant/authorino-operator/main/utils/install.sh | bash -s -- --git-ref main
	kubectl patch deployment/authorino-webhooks -n $(AUTHORINO_OPERATOR_NAMESPACE) -p '{"spec":{"template":{"spec":{"containers":[{"name":"webhooks","image":"$(AUTHORINO_IMAGE)","imagePullPolicy":"IfNotPresent"}]}}}}'
	kubectl -n $(AUTHORINO_OPERATOR_NAMESPACE) wait --timeout=300s --for=condition=Available deployments --all

TLS_ENABLED ?= true
AUTHORINO_INSTANCE ?= authorino
NAMESPACE ?= default
certs: sed ## Requests TLS certificates for the Authorino instance if TLS is enabled, cert-manager.io is installed, and the secret is not already present
ifeq (true,$(TLS_ENABLED))
ifeq (,$(shell kubectl -n $(NAMESPACE) get secret/authorino-oidc-server-cert 2>/dev/null))
	curl -sl https://raw.githubusercontent.com/kuadrant/authorino/main/deploy/certs.yaml | $(SED) "s/\$$(AUTHORINO_INSTANCE)/$(AUTHORINO_INSTANCE)/g;s/\$$(NAMESPACE)/$(NAMESPACE)/g" | kubectl -n $(NAMESPACE) apply -f -
else
	echo "tls cert secret found."
endif
else
	echo "tls disabled."
endif

deploy-authorino: certs sed ## Deploys an instance of Authorino into the Kubernetes cluster configured in ~/.kube/config
	@{ \
	set -e ;\
	TEMP_FILE=/tmp/authorino-deploy-$$(openssl rand -hex 4).yaml ;\
	curl -sl https://raw.githubusercontent.com/kuadrant/authorino/main/deploy/authorino.yaml > $$TEMP_FILE ;\
	$(SED) -i "s/\$$(AUTHORINO_INSTANCE)/$(AUTHORINO_INSTANCE)/g;s/\$$(TLS_ENABLED)/$(TLS_ENABLED)/g" $$TEMP_FILE ;\
	kubectl -n $(NAMESPACE) apply -f $$TEMP_FILE ;\
	kubectl patch -n $(NAMESPACE) authorino/$(AUTHORINO_INSTANCE) --type='merge' -p '{"spec":{"image": "$(AUTHORINO_IMAGE)"}}' ;\
	rm -rf $$TEMP_FILE ;\
	}


##@ Limitador

deploy-limitador:
	kubectl create configmap limits --from-file=$(PROJECT_PATH)/utils/docker-compose/limits.yaml
	kubectl -n $(NAMESPACE) apply -f $(PROJECT_PATH)/utils/deploy/limitador.yaml


##@ User Apps

.PHONY: user-apps


ifeq (true,$(TLS_ENABLED))
ENVOY_OVERLAY = tls
else
ENVOY_OVERLAY = notls
endif
user-apps: ## Deploys talker API and envoy
	kubectl -n $(NAMESPACE) apply -f https://raw.githubusercontent.com/kuadrant/authorino-examples/main/talker-api/talker-api-deploy.yaml
	kubectl -n $(NAMESPACE) apply -f $(PROJECT_PATH)/utils/deploy/envoy-$(ENVOY_OVERLAY).yaml


##@ Util

.PHONY: local-setup local-env-setup local-cleanup local-rollout sed

local-setup: local-env-setup
	kubectl -n $(NAMESPACE) wait --timeout=300s --for=condition=Available deployments --all
	@{ \
	echo "Now you can export the envoy service by doing:"; \
	echo "kubectl port-forward --namespace $(NAMESPACE) deployment/envoy 8000:8000"; \
	echo "After that, you can curl -H \"Host: myhost.com\" localhost:8000"; \
	}

local-env-setup:
	$(MAKE) kind-delete-cluster
	$(MAKE) kind-create-cluster
	$(MAKE) install-authorino-operator
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
