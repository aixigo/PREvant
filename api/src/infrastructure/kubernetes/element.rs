use super::traefik_crds::{IngressRoute, Middleware};
use crate::infrastructure::{APP_NAME_LABEL, CONTAINER_TYPE_LABEL, SERVICE_NAME_LABEL};
use domain::{AppName, app_instance::ContainerType};
use k8s_openapi::{
    Resource as _,
    api::{
        apps::v1::{Deployment, StatefulSet},
        batch::v1::{CronJob, Job},
        core::v1::{ConfigMap, PersistentVolumeClaim, Pod, Secret, Service, ServiceAccount},
        networking::v1::{Ingress, NetworkPolicy},
        rbac::v1::{Role, RoleBinding},
    },
};
use kube::{
    Resource, ResourceExt as _,
    api::{ApiResource, DynamicObject, ObjectMeta},
};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

#[derive(Clone)]
pub enum K8sElement {
    Role(Role),
    RoleBinding(RoleBinding),
    StatefulSet(StatefulSet),
    ConfigMap(ConfigMap),
    Secret(Secret),
    PVC(PersistentVolumeClaim),
    Service(Service),
    Pod(Pod),
    Deployment(Deployment),
    Job(Job),
    CronJob(CronJob),
    ServiceAccount(ServiceAccount),
    Ingress(Ingress),
    NetworkPolicy(NetworkPolicy),
    TraefikIngressRoute(IngressRoute),
    TraefikMiddleware(Middleware),
    Other((DynamicObject, ApiResource)),
}

impl From<Role> for K8sElement {
    fn from(value: Role) -> Self {
        Self::Role(value)
    }
}
impl From<RoleBinding> for K8sElement {
    fn from(value: RoleBinding) -> Self {
        Self::RoleBinding(value)
    }
}
impl From<StatefulSet> for K8sElement {
    fn from(value: StatefulSet) -> Self {
        Self::StatefulSet(value)
    }
}
impl From<ConfigMap> for K8sElement {
    fn from(value: ConfigMap) -> Self {
        Self::ConfigMap(value)
    }
}
impl From<Secret> for K8sElement {
    fn from(value: Secret) -> Self {
        Self::Secret(value)
    }
}
impl From<PersistentVolumeClaim> for K8sElement {
    fn from(value: PersistentVolumeClaim) -> Self {
        Self::PVC(value)
    }
}
impl From<Service> for K8sElement {
    fn from(value: Service) -> Self {
        Self::Service(value)
    }
}
impl From<Pod> for K8sElement {
    fn from(value: Pod) -> Self {
        Self::Pod(value)
    }
}
impl From<Deployment> for K8sElement {
    fn from(value: Deployment) -> Self {
        Self::Deployment(value)
    }
}
impl From<Job> for K8sElement {
    fn from(value: Job) -> Self {
        Self::Job(value)
    }
}
impl From<CronJob> for K8sElement {
    fn from(value: CronJob) -> Self {
        Self::CronJob(value)
    }
}
impl From<ServiceAccount> for K8sElement {
    fn from(value: ServiceAccount) -> Self {
        Self::ServiceAccount(value)
    }
}
impl From<Ingress> for K8sElement {
    fn from(value: Ingress) -> Self {
        Self::Ingress(value)
    }
}
impl From<NetworkPolicy> for K8sElement {
    fn from(value: NetworkPolicy) -> Self {
        Self::NetworkPolicy(value)
    }
}
impl From<IngressRoute> for K8sElement {
    fn from(value: IngressRoute) -> Self {
        Self::TraefikIngressRoute(value)
    }
}
impl From<Middleware> for K8sElement {
    fn from(value: Middleware) -> Self {
        Self::TraefikMiddleware(value)
    }
}
impl From<(DynamicObject, ApiResource)> for K8sElement {
    fn from(value: (DynamicObject, ApiResource)) -> Self {
        Self::Other(value)
    }
}

impl K8sElement {
    pub fn is_well_known(api_resource: &ApiResource) -> bool {
        match (
            api_resource.group.as_str(),
            api_resource.version.as_str(),
            api_resource.kind.as_str(),
        ) {
            (Role::GROUP, Role::VERSION, Role::KIND)
            | (RoleBinding::GROUP, RoleBinding::VERSION, RoleBinding::KIND)
            | (StatefulSet::GROUP, StatefulSet::VERSION, StatefulSet::KIND)
            | (ConfigMap::GROUP, ConfigMap::VERSION, ConfigMap::KIND)
            | (Secret::GROUP, Secret::VERSION, Secret::KIND)
            | (
                PersistentVolumeClaim::GROUP,
                PersistentVolumeClaim::VERSION,
                PersistentVolumeClaim::KIND,
            )
            | (Service::GROUP, Service::VERSION, Service::KIND)
            | (Pod::GROUP, Pod::VERSION, Pod::KIND)
            | (Deployment::GROUP, Deployment::VERSION, Deployment::KIND)
            | (Job::GROUP, Job::VERSION, Job::KIND)
            | (CronJob::GROUP, CronJob::VERSION, CronJob::KIND)
            | (ServiceAccount::GROUP, ServiceAccount::VERSION, ServiceAccount::KIND)
            | (Ingress::GROUP, Ingress::VERSION, Ingress::KIND)
            | (NetworkPolicy::GROUP, NetworkPolicy::VERSION, NetworkPolicy::KIND) => true,

            (group, version, kind)
                if group == IngressRoute::group(&())
                    && version == IngressRoute::version(&())
                    && kind == IngressRoute::kind(&()) =>
            {
                true
            }
            (group, version, kind)
                if group == Middleware::group(&())
                    && version == Middleware::version(&())
                    && kind == Middleware::kind(&()) =>
            {
                true
            }
            _ => false,
        }
    }

    pub fn metadata(&self) -> &ObjectMeta {
        match self {
            K8sElement::Role(role) => &role.metadata,
            K8sElement::RoleBinding(role_binding) => &role_binding.metadata,
            K8sElement::StatefulSet(stateful_set) => &stateful_set.metadata,
            K8sElement::ConfigMap(config_map) => &config_map.metadata,
            K8sElement::Secret(secret) => &secret.metadata,
            K8sElement::PVC(persistent_volume_claim) => &persistent_volume_claim.metadata,
            K8sElement::Service(service) => &service.metadata,
            K8sElement::Pod(pod) => &pod.metadata,
            K8sElement::Deployment(deployment) => &deployment.metadata,
            K8sElement::Job(job) => &job.metadata,
            K8sElement::CronJob(cron_job) => &cron_job.metadata,
            K8sElement::ServiceAccount(service_account) => &service_account.metadata,
            K8sElement::Ingress(ingress) => &ingress.metadata,
            K8sElement::NetworkPolicy(network_policy) => &network_policy.metadata,
            K8sElement::TraefikIngressRoute(ingress_route) => &ingress_route.metadata,
            K8sElement::TraefikMiddleware(middleware) => &middleware.metadata,
            K8sElement::Other((dynamic_object, _)) => &dynamic_object.metadata,
        }
    }

    pub fn with_prevant_metadata(mut self, app_name: &AppName) -> Self {
        let metadata = match &mut self {
            K8sElement::Role(role) => &mut role.metadata,
            K8sElement::RoleBinding(role_binding) => &mut role_binding.metadata,
            K8sElement::StatefulSet(stateful_set) => &mut stateful_set.metadata,
            K8sElement::ConfigMap(config_map) => &mut config_map.metadata,
            K8sElement::Secret(secret) => &mut secret.metadata,
            K8sElement::PVC(persistent_volume_claim) => &mut persistent_volume_claim.metadata,
            K8sElement::Service(service) => &mut service.metadata,
            K8sElement::Pod(pod) => &mut pod.metadata,
            K8sElement::Deployment(deployment) => &mut deployment.metadata,
            K8sElement::Job(job) => &mut job.metadata,
            K8sElement::CronJob(cron_job) => &mut cron_job.metadata,
            K8sElement::ServiceAccount(service_account) => &mut service_account.metadata,
            K8sElement::Ingress(ingress) => &mut ingress.metadata,
            K8sElement::NetworkPolicy(network_policy) => &mut network_policy.metadata,
            K8sElement::TraefikIngressRoute(ingress_route) => &mut ingress_route.metadata,
            K8sElement::TraefikMiddleware(middleware) => &mut middleware.metadata,
            K8sElement::Other((dynamic_object, _)) => &mut dynamic_object.metadata,
        };

        metadata.namespace = Some(app_name.to_rfc1123_namespace_id());
        metadata.labels = match metadata.labels.take() {
            Some(mut labels) => {
                labels.insert(APP_NAME_LABEL.to_string(), app_name.to_string());
                Some(labels)
            }
            None => Some(BTreeMap::from([(
                APP_NAME_LABEL.to_string(),
                app_name.to_string(),
            )])),
        };

        if let Self::Deployment(deployment) = &mut self {
            // TODO: eventually we should support that bootstrapped elements support the
            // reading of PREvant labels. In this case, the  following code snippet
            // should be evaluated.
            // let service_name = deployment
            //     .labels()
            //     .get(SERVICE_NAME_LABEL)
            //     .cloned()
            //     .unwrap_or_else(|| {
            //         deployment.metadata.name.clone().unwrap_or_default()
            //     });
            let service_name = deployment.metadata.name.clone().unwrap_or_default();

            deployment
                .labels_mut()
                .entry(SERVICE_NAME_LABEL.to_string())
                .or_insert(service_name);
            deployment
                .labels_mut()
                .entry(CONTAINER_TYPE_LABEL.to_string())
                .or_insert(ContainerType::ApplicationCompanion.to_string());
        }

        self
    }
}

impl<'de> Deserialize<'de> for K8sElement {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut dy = DynamicObject::deserialize(deserializer)?;

        let api_version = dy
            .types
            .as_ref()
            .map(|t| t.api_version.as_str())
            .unwrap_or_default();
        let kind = dy
            .types
            .as_ref()
            .map(|t| t.kind.as_str())
            .unwrap_or_default();

        match (api_version, kind) {
            (Role::API_VERSION, Role::KIND) => Ok(Self::Role(
                dy.try_parse::<Role>().map_err(serde::de::Error::custom)?,
            )),
            (RoleBinding::API_VERSION, RoleBinding::KIND) => Ok(Self::RoleBinding(
                dy.try_parse::<RoleBinding>()
                    .map_err(serde::de::Error::custom)?,
            )),
            (StatefulSet::API_VERSION, StatefulSet::KIND) => Ok(Self::StatefulSet(
                dy.try_parse::<StatefulSet>()
                    .map_err(serde::de::Error::custom)?,
            )),
            (ConfigMap::API_VERSION, ConfigMap::KIND) => Ok(Self::ConfigMap(
                dy.try_parse::<ConfigMap>()
                    .map_err(serde::de::Error::custom)?,
            )),
            (Secret::API_VERSION, Secret::KIND) => {
                if let serde_json::Value::Object(obj) = &mut dy.data {
                    obj.entry("data").and_modify(|obj| {
                        if let serde_json::Value::Object(obj) = obj {
                            for (_k, v) in obj.iter_mut() {
                                if let serde_json::Value::String(str) = v {
                                    // replacing new lines here because it is assumed
                                    // that the data is base64 encoded and thus there
                                    // must be no new lines
                                    *v = str.replace('\n', "").into();
                                }
                            }
                        }
                    });
                }
                Ok(Self::Secret(
                    dy.try_parse::<Secret>().map_err(serde::de::Error::custom)?,
                ))
            }
            (PersistentVolumeClaim::API_VERSION, PersistentVolumeClaim::KIND) => Ok(Self::PVC(
                dy.try_parse::<PersistentVolumeClaim>()
                    .map_err(serde::de::Error::custom)?,
            )),
            (Service::API_VERSION, Service::KIND) => Ok(Self::Service(
                dy.try_parse::<Service>()
                    .map_err(serde::de::Error::custom)?,
            )),
            (Deployment::API_VERSION, Deployment::KIND) => Ok(Self::Deployment(
                dy.try_parse::<Deployment>()
                    .map_err(serde::de::Error::custom)?,
            )),
            (Pod::API_VERSION, Pod::KIND) => Ok(Self::Pod(
                dy.try_parse::<Pod>().map_err(serde::de::Error::custom)?,
            )),
            (Job::API_VERSION, Job::KIND) => Ok(Self::Job(
                dy.try_parse::<Job>().map_err(serde::de::Error::custom)?,
            )),
            (CronJob::API_VERSION, CronJob::KIND) => Ok(Self::CronJob(
                dy.try_parse::<CronJob>()
                    .map_err(serde::de::Error::custom)?,
            )),
            (ServiceAccount::API_VERSION, ServiceAccount::KIND) => Ok(Self::ServiceAccount(
                dy.try_parse::<ServiceAccount>()
                    .map_err(serde::de::Error::custom)?,
            )),
            (Ingress::API_VERSION, Ingress::KIND) => Ok(Self::Ingress(
                dy.try_parse::<Ingress>()
                    .map_err(serde::de::Error::custom)?,
            )),
            (NetworkPolicy::API_VERSION, NetworkPolicy::KIND) => Ok(Self::NetworkPolicy(
                dy.try_parse::<NetworkPolicy>()
                    .map_err(serde::de::Error::custom)?,
            )),
            (api_version, kind)
                if api_version == Middleware::api_version(&()) && kind == Middleware::kind(&()) =>
            {
                Ok(Self::TraefikMiddleware(
                    dy.try_parse::<Middleware>()
                        .map_err(serde::de::Error::custom)?,
                ))
            }
            (api_version, kind)
                if api_version == IngressRoute::api_version(&())
                    && kind == IngressRoute::kind(&()) =>
            {
                Ok(Self::TraefikIngressRoute(
                    dy.try_parse::<IngressRoute>()
                        .map_err(serde::de::Error::custom)?,
                ))
            }
            (api_version, kind) => Err(serde::de::Error::custom(format!(
                "Unknown variant for Kubernetes API object: {api_version}/{kind}, supported are: {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, or {}/{}",
                Role::API_VERSION,
                Role::KIND,
                RoleBinding::API_VERSION,
                RoleBinding::KIND,
                StatefulSet::API_VERSION,
                StatefulSet::KIND,
                ConfigMap::API_VERSION,
                ConfigMap::KIND,
                Secret::API_VERSION,
                Secret::KIND,
                PersistentVolumeClaim::API_VERSION,
                PersistentVolumeClaim::KIND,
                Service::API_VERSION,
                Service::KIND,
                Deployment::API_VERSION,
                Deployment::KIND,
                Pod::API_VERSION,
                Pod::KIND,
                Job::API_VERSION,
                Job::KIND,
                CronJob::API_VERSION,
                CronJob::KIND,
                ServiceAccount::API_VERSION,
                ServiceAccount::KIND,
                Ingress::API_VERSION,
                Ingress::KIND,
                NetworkPolicy::API_VERSION,
                NetworkPolicy::KIND,
                IngressRoute::api_version(&()),
                IngressRoute::kind(&()),
                Middleware::api_version(&()),
                Middleware::kind(&()),
            ))),
        }
    }
}

/// Cleared out Kubernetes object that can be put into the backup.
///
/// For example, the volume mounts should be kept on the Kubernetes infrastructure because they
/// might contain data that a tester of the application crafted for a long time and this should be
/// preserved. Therefore, [`ConfigMap`], [`PersistentVolumeClaim`], and [`Secret`] won't be member
/// of this enum.
///
/// The [`Job`] won't be contained in the back-up because “Jobs represent one-off tasks that
/// run to completion and then stop.” If they are part of the back-up they would restart and
/// try to the same thing again which might be already done.
#[derive(Clone, Debug, PartialEq)]
pub enum K8sBackupElement {
    Role(Role),
    RoleBinding(RoleBinding),
    StatefulSet(StatefulSet),
    Service(Service),
    Pod(Pod),
    Deployment(Deployment),
    CronJob(CronJob),
    ServiceAccount(ServiceAccount),
    NetworkPolicy(NetworkPolicy),
    TraefikIngressRoute(IngressRoute),
    TraefikMiddleware(Middleware),
}

impl Serialize for K8sBackupElement {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            K8sBackupElement::Role(role) => role.serialize(serializer),
            K8sBackupElement::RoleBinding(role_binding) => role_binding.serialize(serializer),
            K8sBackupElement::StatefulSet(stateful_set) => stateful_set.serialize(serializer),
            K8sBackupElement::Service(service) => service.serialize(serializer),
            K8sBackupElement::Pod(pod) => pod.serialize(serializer),
            K8sBackupElement::Deployment(deployment) => deployment.serialize(serializer),
            K8sBackupElement::CronJob(cron_job) => cron_job.serialize(serializer),
            K8sBackupElement::ServiceAccount(service_account) => {
                service_account.serialize(serializer)
            }
            K8sBackupElement::NetworkPolicy(network_policy) => network_policy.serialize(serializer),
            K8sBackupElement::TraefikIngressRoute(ingress_route) => {
                ingress_route.serialize(serializer)
            }
            K8sBackupElement::TraefikMiddleware(middleware) => middleware.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for K8sBackupElement {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let dy = DynamicObject::deserialize(deserializer)?;

        let api_version = dy
            .types
            .as_ref()
            .map(|t| t.api_version.as_str())
            .unwrap_or_default();
        let kind = dy
            .types
            .as_ref()
            .map(|t| t.kind.as_str())
            .unwrap_or_default();

        match (api_version, kind) {
            (Role::API_VERSION, Role::KIND) => Ok(Self::Role(
                dy.try_parse::<Role>().map_err(serde::de::Error::custom)?,
            )),
            (RoleBinding::API_VERSION, RoleBinding::KIND) => Ok(Self::RoleBinding(
                dy.try_parse::<RoleBinding>()
                    .map_err(serde::de::Error::custom)?,
            )),
            (StatefulSet::API_VERSION, StatefulSet::KIND) => Ok(Self::StatefulSet(
                dy.try_parse::<StatefulSet>()
                    .map_err(serde::de::Error::custom)?,
            )),
            (Service::API_VERSION, Service::KIND) => Ok(Self::Service(
                dy.try_parse::<Service>()
                    .map_err(serde::de::Error::custom)?,
            )),
            (Deployment::API_VERSION, Deployment::KIND) => {
                let mut deployment = dy
                    .try_parse::<Deployment>()
                    .map_err(serde::de::Error::custom)?;
                // TODO: eventually we should support that bootstrapped elements support the
                // reading of PREvant labels. In this case, the  following code snippet
                // should be evaluated.
                // let service_name = deployment
                //     .labels()
                //     .get(SERVICE_NAME_LABEL)
                //     .cloned()
                //     .unwrap_or_else(|| {
                //         deployment.metadata.name.clone().unwrap_or_default()
                //     });
                let service_name = deployment.metadata.name.clone().unwrap_or_default();

                deployment
                    .labels_mut()
                    .entry(SERVICE_NAME_LABEL.to_string())
                    .or_insert(service_name);
                deployment
                    .labels_mut()
                    .entry(CONTAINER_TYPE_LABEL.to_string())
                    .or_insert(ContainerType::ApplicationCompanion.to_string());

                Ok(Self::Deployment(deployment))
            }
            (Pod::API_VERSION, Pod::KIND) => Ok(Self::Pod(
                dy.try_parse::<Pod>().map_err(serde::de::Error::custom)?,
            )),
            (CronJob::API_VERSION, CronJob::KIND) => Ok(Self::CronJob(
                dy.try_parse::<CronJob>()
                    .map_err(serde::de::Error::custom)?,
            )),
            (ServiceAccount::API_VERSION, ServiceAccount::KIND) => Ok(Self::ServiceAccount(
                dy.try_parse::<ServiceAccount>()
                    .map_err(serde::de::Error::custom)?,
            )),
            (NetworkPolicy::API_VERSION, NetworkPolicy::KIND) => Ok(Self::NetworkPolicy(
                dy.try_parse::<NetworkPolicy>()
                    .map_err(serde::de::Error::custom)?,
            )),
            (api_version, kind)
                if api_version == Middleware::api_version(&()) && kind == Middleware::kind(&()) =>
            {
                Ok(Self::TraefikMiddleware(
                    dy.try_parse::<Middleware>()
                        .map_err(serde::de::Error::custom)?,
                ))
            }
            (api_version, kind)
                if api_version == IngressRoute::api_version(&())
                    && kind == IngressRoute::kind(&()) =>
            {
                Ok(Self::TraefikIngressRoute(
                    dy.try_parse::<IngressRoute>()
                        .map_err(serde::de::Error::custom)?,
                ))
            }
            (api_version, kind) => Err(serde::de::Error::custom(format!(
                "Unknown variant for back-up payload: {api_version}/{kind}, supported are: {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, {}/{}, or {}/{}",
                Role::API_VERSION,
                Role::KIND,
                RoleBinding::API_VERSION,
                RoleBinding::KIND,
                StatefulSet::API_VERSION,
                StatefulSet::KIND,
                Service::API_VERSION,
                Service::KIND,
                Deployment::API_VERSION,
                Deployment::KIND,
                Pod::API_VERSION,
                Pod::KIND,
                CronJob::API_VERSION,
                CronJob::KIND,
                ServiceAccount::API_VERSION,
                ServiceAccount::KIND,
                NetworkPolicy::API_VERSION,
                NetworkPolicy::KIND,
                IngressRoute::api_version(&()),
                IngressRoute::kind(&()),
                Middleware::api_version(&()),
                Middleware::kind(&()),
            ))),
        }
    }
}

impl From<K8sBackupElement> for K8sElement {
    fn from(value: K8sBackupElement) -> Self {
        match value {
            K8sBackupElement::Role(role) => Self::Role(role),
            K8sBackupElement::RoleBinding(role_binding) => Self::RoleBinding(role_binding),
            K8sBackupElement::StatefulSet(stateful_set) => Self::StatefulSet(stateful_set),
            K8sBackupElement::Service(service) => Self::Service(service),
            K8sBackupElement::Pod(pod) => Self::Pod(pod),
            K8sBackupElement::Deployment(deployment) => Self::Deployment(deployment),
            K8sBackupElement::CronJob(cron_job) => Self::CronJob(cron_job),
            K8sBackupElement::ServiceAccount(service_account) => {
                Self::ServiceAccount(service_account)
            }
            K8sBackupElement::NetworkPolicy(network_policy) => Self::NetworkPolicy(network_policy),
            K8sBackupElement::TraefikIngressRoute(ingress_route) => {
                Self::TraefikIngressRoute(ingress_route)
            }
            K8sBackupElement::TraefikMiddleware(middleware) => Self::TraefikMiddleware(middleware),
        }
    }
}

#[cfg(test)]
mod tests {
    mod deserialize_role {
        use super::super::*;

        #[test]
        fn element() {
            let yaml = r#"
                apiVersion: rbac.authorization.k8s.io/v1
                kind: Role
                metadata:
                  name: my-role
                rules:
                  - apiGroups: [""]
                    resources: ["pods"]
                    verbs: ["get", "list"]
                "#;

            let element = noyalib::from_str::<K8sElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sElement::Role(e) if e.metadata.name.as_deref() == Some("my-role")
            ));
        }

        #[test]
        fn backup_element() {
            let yaml = r#"
                apiVersion: rbac.authorization.k8s.io/v1
                kind: Role
                metadata:
                  name: my-role
                rules:
                  - apiGroups: [""]
                    resources: ["pods"]
                    verbs: ["get", "list"]
                "#;

            let element = noyalib::from_str::<K8sBackupElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sBackupElement::Role(e) if e.metadata.name.as_deref() == Some("my-role")
            ));
        }
    }

    mod deserialize_role_binding {
        use super::super::*;

        #[test]
        fn element() {
            let yaml = r#"
                apiVersion: rbac.authorization.k8s.io/v1
                kind: RoleBinding
                metadata:
                  name: my-role-binding
                roleRef:
                  apiGroup: rbac.authorization.k8s.io
                  kind: Role
                  name: my-role
                subjects:
                  - kind: ServiceAccount
                    name: my-sa
                "#;

            let element = noyalib::from_str::<K8sElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sElement::RoleBinding(e) if e.metadata.name.as_deref() == Some("my-role-binding")
            ));
        }

        #[test]
        fn backup_element() {
            let yaml = r#"
                apiVersion: rbac.authorization.k8s.io/v1
                kind: RoleBinding
                metadata:
                  name: my-role-binding
                roleRef:
                  apiGroup: rbac.authorization.k8s.io
                  kind: Role
                  name: my-role
                subjects:
                  - kind: ServiceAccount
                    name: my-sa
                "#;

            let element = noyalib::from_str::<K8sBackupElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sBackupElement::RoleBinding(e) if e.metadata.name.as_deref() == Some("my-role-binding")
            ));
        }
    }

    mod deserialize_stateful_set {
        use super::super::*;

        #[test]
        fn element() {
            let yaml = r#"
                apiVersion: apps/v1
                kind: StatefulSet
                metadata:
                  name: my-stateful-set
                spec:
                  selector:
                    matchLabels:
                      app: my-app
                  serviceName: my-service
                  template:
                    metadata:
                      labels:
                        app: my-app
                    spec:
                      containers:
                        - name: my-container
                          image: nginx
                "#;

            let element = noyalib::from_str::<K8sElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sElement::StatefulSet(e) if e.metadata.name.as_deref() == Some("my-stateful-set")
            ));
        }

        #[test]
        fn backup_element() {
            let yaml = r#"
                apiVersion: apps/v1
                kind: StatefulSet
                metadata:
                  name: my-stateful-set
                spec:
                  selector:
                    matchLabels:
                      app: my-app
                  serviceName: my-service
                  template:
                    metadata:
                      labels:
                        app: my-app
                    spec:
                      containers:
                        - name: my-container
                          image: nginx
                "#;

            let element = noyalib::from_str::<K8sBackupElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sBackupElement::StatefulSet(e) if e.metadata.name.as_deref() == Some("my-stateful-set")
            ));
        }
    }

    mod deserialize_config_map {
        use super::super::*;

        #[test]
        fn element() {
            let yaml = r#"
                apiVersion: v1
                kind: ConfigMap
                metadata:
                  name: my-config
                data:
                  key: value
                "#;

            let element = noyalib::from_str::<K8sElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sElement::ConfigMap(e) if e.metadata.name.as_deref() == Some("my-config")
            ));
        }
    }

    mod deserialize_secret {
        use super::super::*;

        #[test]
        fn element() {
            let yaml = r#"
                apiVersion: v1
                kind: Secret
                metadata:
                  name: my-secret
                data:
                  password: cGFzc3dvcmQ=
                "#;

            let element = noyalib::from_str::<K8sElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sElement::Secret(e) if e.metadata.name.as_deref() == Some("my-secret")
            ));
        }
    }

    mod deserialize_pvc {
        use super::super::*;

        #[test]
        fn element() {
            let yaml = r#"
                apiVersion: v1
                kind: PersistentVolumeClaim
                metadata:
                  name: my-pvc
                spec:
                  accessModes:
                    - ReadWriteOnce
                  resources:
                    requests:
                      storage: 1Gi
                "#;

            let element = noyalib::from_str::<K8sElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sElement::PVC(e) if e.metadata.name.as_deref() == Some("my-pvc")
            ));
        }
    }

    mod deserialize_service {
        use super::super::*;

        #[test]
        fn element() {
            let yaml = r#"
                apiVersion: v1
                kind: Service
                metadata:
                  name: my-service
                spec:
                  ports:
                    - port: 80
                      targetPort: 8080
                  selector:
                    app: my-app
                "#;

            let element = noyalib::from_str::<K8sElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sElement::Service(e) if e.metadata.name.as_deref() == Some("my-service")
            ));
        }

        #[test]
        fn backup_element() {
            let yaml = r#"
                apiVersion: v1
                kind: Service
                metadata:
                  name: my-service
                spec:
                  ports:
                    - port: 80
                      targetPort: 8080
                  selector:
                    app: my-app
                "#;

            let element = noyalib::from_str::<K8sBackupElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sBackupElement::Service(e) if e.metadata.name.as_deref() == Some("my-service")
            ));
        }
    }

    mod deserialize_pod {
        use super::super::*;

        #[test]
        fn element() {
            let yaml = r#"
                apiVersion: v1
                kind: Pod
                metadata:
                  name: my-pod
                spec:
                  containers:
                    - name: my-container
                      image: nginx
                "#;

            let element = noyalib::from_str::<K8sElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sElement::Pod(e) if e.metadata.name.as_deref() == Some("my-pod")
            ));
        }

        #[test]
        fn backup_element() {
            let yaml = r#"
                apiVersion: v1
                kind: Pod
                metadata:
                  name: my-pod
                spec:
                  containers:
                    - name: my-container
                      image: nginx
                "#;

            let element = noyalib::from_str::<K8sBackupElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sBackupElement::Pod(e) if e.metadata.name.as_deref() == Some("my-pod")
            ));
        }
    }

    mod deserialize_deployment {
        use super::super::*;

        #[test]
        fn element() {
            let yaml = r#"
                apiVersion: apps/v1
                kind: Deployment
                metadata:
                  name: my-deployment
                spec:
                  selector:
                    matchLabels:
                      app: my-app
                  template:
                    metadata:
                      labels:
                        app: my-app
                    spec:
                      containers:
                        - name: my-container
                          image: nginx
                "#;

            let element = noyalib::from_str::<K8sElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sElement::Deployment(e) if e.metadata.name.as_deref() == Some("my-deployment")
            ));
        }

        #[test]
        fn backup_element() {
            let yaml = r#"
                apiVersion: apps/v1
                kind: Deployment
                metadata:
                  name: my-deployment
                spec:
                  selector:
                    matchLabels:
                      app: my-app
                  template:
                    metadata:
                      labels:
                        app: my-app
                    spec:
                      containers:
                        - name: my-container
                          image: nginx
                "#;

            let element = noyalib::from_str::<K8sBackupElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sBackupElement::Deployment(e) if e.metadata.name.as_deref() == Some("my-deployment")
            ));
        }
    }

    mod deserialize_job {
        use super::super::*;

        #[test]
        fn element() {
            let yaml = r#"
                apiVersion: batch/v1
                kind: Job
                metadata:
                  name: my-job
                spec:
                  template:
                    spec:
                      containers:
                        - name: my-container
                          image: busybox
                          command: ["echo", "hello"]
                      restartPolicy: Never
                "#;

            let element = noyalib::from_str::<K8sElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sElement::Job(e) if e.metadata.name.as_deref() == Some("my-job")
            ));
        }
    }

    mod deserialize_cron_job {
        use super::super::*;

        #[test]
        fn element() {
            let yaml = r#"
                apiVersion: batch/v1
                kind: CronJob
                metadata:
                  name: my-cron-job
                spec:
                  schedule: "*/5 * * * *"
                  jobTemplate:
                    spec:
                      template:
                        spec:
                          containers:
                            - name: my-container
                              image: busybox
                              command: ["echo", "hello"]
                          restartPolicy: Never
                "#;

            let element = noyalib::from_str::<K8sElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sElement::CronJob(e) if e.metadata.name.as_deref() == Some("my-cron-job")
            ));
        }

        #[test]
        fn backup_element() {
            let yaml = r#"
                apiVersion: batch/v1
                kind: CronJob
                metadata:
                  name: my-cron-job
                spec:
                  schedule: "*/5 * * * *"
                  jobTemplate:
                    spec:
                      template:
                        spec:
                          containers:
                            - name: my-container
                              image: busybox
                              command: ["echo", "hello"]
                          restartPolicy: Never
                "#;

            let element = noyalib::from_str::<K8sBackupElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sBackupElement::CronJob(e) if e.metadata.name.as_deref() == Some("my-cron-job")
            ));
        }
    }

    mod deserialize_service_account {
        use super::super::*;

        #[test]
        fn element() {
            let yaml = r#"
                apiVersion: v1
                kind: ServiceAccount
                metadata:
                  name: my-sa
                "#;

            let element = noyalib::from_str::<K8sElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sElement::ServiceAccount(e) if e.metadata.name.as_deref() == Some("my-sa")
            ));
        }

        #[test]
        fn backup_element() {
            let yaml = r#"
                apiVersion: v1
                kind: ServiceAccount
                metadata:
                  name: my-sa
                "#;

            let element = noyalib::from_str::<K8sBackupElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sBackupElement::ServiceAccount(e) if e.metadata.name.as_deref() == Some("my-sa")
            ));
        }
    }

    mod deserialize_ingress {
        use super::super::*;

        #[test]
        fn element() {
            let yaml = r#"
                apiVersion: networking.k8s.io/v1
                kind: Ingress
                metadata:
                  name: my-ingress
                spec:
                  rules:
                    - host: example.com
                      http:
                        paths:
                          - path: /
                            pathType: Prefix
                            backend:
                              service:
                                name: my-service
                                port:
                                  number: 80
                "#;

            let element = noyalib::from_str::<K8sElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sElement::Ingress(e) if e.metadata.name.as_deref() == Some("my-ingress")
            ));
        }
    }

    mod deserialize_network_policy {
        use super::super::*;

        #[test]
        fn element() {
            let yaml = r#"
                apiVersion: networking.k8s.io/v1
                kind: NetworkPolicy
                metadata:
                  name: my-network-policy
                spec:
                  podSelector:
                    matchLabels:
                      app: my-app
                  ingress:
                    - from:
                        - podSelector: {}
                "#;

            let element = noyalib::from_str::<K8sElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sElement::NetworkPolicy(e) if e.metadata.name.as_deref() == Some("my-network-policy")
            ));
        }

        #[test]
        fn backup_element() {
            let yaml = r#"
                apiVersion: networking.k8s.io/v1
                kind: NetworkPolicy
                metadata:
                  name: my-network-policy
                spec:
                  podSelector:
                    matchLabels:
                      app: my-app
                  ingress:
                    - from:
                        - podSelector: {}
                "#;

            let element = noyalib::from_str::<K8sBackupElement>(yaml).unwrap();

            assert!(matches!(
                element,
                K8sBackupElement::NetworkPolicy(e) if e.metadata.name.as_deref() == Some("my-network-policy")
            ));
        }
    }
}
