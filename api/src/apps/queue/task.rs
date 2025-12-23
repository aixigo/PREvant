use crate::models::{
    user_defined_parameters::UserDefinedParameters, AppName, AppStatusChangeId, Owner,
    ServiceConfig,
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq)]
#[serde(untagged)]
pub(super) enum AppTask {
    MovePayloadToBackUpAndDeleteFromInfrastructure {
        status_id: AppStatusChangeId,
        app_name: AppName,
        infrastructure_payload_to_back_up: Vec<serde_json::Value>,
    },
    RestoreOnInfrastructureAndDeleteFromBackup {
        status_id: AppStatusChangeId,
        app_name: AppName,
        infrastructure_payload_to_restore: Vec<serde_json::Value>,
    },
    CreateOrUpdate {
        app_name: AppName,
        status_id: AppStatusChangeId,
        replicate_from: Option<AppName>,
        service_configs: Vec<ServiceConfig>,
        owners: Vec<Owner>,
        user_defined_parameters: Option<serde_json::Value>,
    },
    Delete {
        status_id: AppStatusChangeId,
        app_name: AppName,
    },
}

impl AppTask {
    pub fn app_name(&self) -> &AppName {
        match self {
            AppTask::CreateOrUpdate { app_name, .. } => app_name,
            AppTask::Delete { app_name, .. } => app_name,
            AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure { app_name, .. } => app_name,
            AppTask::RestoreOnInfrastructureAndDeleteFromBackup { app_name, .. } => app_name,
        }
    }
    pub fn status_id(&self) -> &AppStatusChangeId {
        match self {
            AppTask::CreateOrUpdate { status_id, .. } => status_id,
            AppTask::Delete { status_id, .. } => status_id,
            AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure { status_id, .. } => status_id,
            AppTask::RestoreOnInfrastructureAndDeleteFromBackup { status_id, .. } => status_id,
        }
    }

    pub fn merge_with(self, other: AppTask) -> Self {
        assert_eq!(self.app_name(), other.app_name());
        match (self, other) {
            (
                Self::CreateOrUpdate {
                    service_configs,
                    owners,
                    user_defined_parameters,
                    ..
                },
                Self::CreateOrUpdate {
                    app_name,
                    status_id,
                    replicate_from,
                    service_configs: o_service_configs,
                    owners: o_owners,
                    user_defined_parameters: o_user_defined_parameters,
                    ..
                },
            ) => {
                let mut configs = service_configs
                    .into_iter()
                    .map(|sc| (sc.service_name().clone(), sc))
                    .collect::<HashMap<_, _>>();

                for sc in o_service_configs.into_iter() {
                    match configs.get_mut(sc.service_name()) {
                        Some(existing_sc) => {
                            *existing_sc = sc.merge_with(existing_sc.clone());
                        }
                        None => {
                            configs.insert(sc.service_name().clone(), sc);
                        }
                    }
                }

                let mut service_configs = configs.into_values().collect::<Vec<_>>();
                service_configs
                    .sort_unstable_by(|sc1, sc2| sc1.service_name().cmp(sc2.service_name()));

                let mut owners = Owner::normalize(HashSet::from_iter(
                    owners.into_iter().chain(o_owners.into_iter()),
                ))
                .into_iter()
                .collect::<Vec<_>>();
                owners.sort_unstable_by(|o1, o2| o1.sub.cmp(&o2.sub));

                Self::CreateOrUpdate {
                    app_name,
                    status_id,
                    replicate_from,
                    service_configs,
                    owners,
                    user_defined_parameters: match (
                        user_defined_parameters,
                        o_user_defined_parameters,
                    ) {
                        (None, None) => None,
                        (None, Some(value)) => Some(value),
                        (Some(value), None) => Some(value),
                        (Some(mut value), Some(other)) => {
                            UserDefinedParameters::merge_json(&mut value, other);
                            Some(value)
                        }
                    },
                }
            }
            (
                Self::CreateOrUpdate { .. },
                Self::Delete {
                    status_id,
                    app_name,
                },
            ) => Self::Delete {
                status_id,
                app_name,
            },
            (
                Self::Delete { .. },
                Self::CreateOrUpdate {
                    app_name,
                    status_id,
                    replicate_from,
                    service_configs,
                    owners,
                    user_defined_parameters,
                },
            ) => Self::CreateOrUpdate {
                app_name,
                status_id,
                replicate_from,
                service_configs,
                owners,
                user_defined_parameters,
            },
            (
                Self::Delete { .. },
                Self::Delete {
                    status_id,
                    app_name,
                },
            ) => Self::Delete {
                status_id,
                app_name,
            },
            _ => unimplemented!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sc;
    use openidconnect::{IssuerUrl, SubjectIdentifier};

    #[test]
    fn merge_delete_with_delete() {
        let t1 = AppTask::Delete {
            status_id: AppStatusChangeId::new(),
            app_name: AppName::master(),
        };
        let status_id_2 = AppStatusChangeId::new();
        let t2 = AppTask::Delete {
            status_id: status_id_2,
            app_name: AppName::master(),
        };

        let merged = t1.merge_with(t2);

        assert_eq!(
            merged,
            AppTask::Delete {
                status_id: status_id_2,
                app_name: AppName::master(),
            },
        );
    }

    #[test]
    fn merge_delete_with_create_or_update() {
        let t1 = AppTask::Delete {
            status_id: AppStatusChangeId::new(),
            app_name: AppName::master(),
        };
        let status_id_2 = AppStatusChangeId::new();
        let t2 = AppTask::CreateOrUpdate {
            status_id: status_id_2,
            app_name: AppName::master(),
            replicate_from: None,
            service_configs: vec![sc!("nginx")],
            owners: Vec::new(),
            user_defined_parameters: None,
        };

        let merged = t1.merge_with(t2);

        assert_eq!(
            merged,
            AppTask::CreateOrUpdate {
                status_id: status_id_2,
                app_name: AppName::master(),
                replicate_from: None,
                service_configs: vec![sc!("nginx")],
                owners: Vec::new(),
                user_defined_parameters: None,
            },
        );
    }

    #[test]
    fn merge_create_or_update_with_delete() {
        let t1 = AppTask::CreateOrUpdate {
            status_id: AppStatusChangeId::new(),
            app_name: AppName::master(),
            replicate_from: None,
            service_configs: vec![sc!("nginx")],
            owners: Vec::new(),
            user_defined_parameters: None,
        };
        let status_id_2 = AppStatusChangeId::new();
        let t2 = AppTask::Delete {
            status_id: status_id_2,
            app_name: AppName::master(),
        };

        let merged = t1.merge_with(t2);

        assert_eq!(
            merged,
            AppTask::Delete {
                status_id: status_id_2,
                app_name: AppName::master(),
            },
        );
    }

    #[test]
    fn merge_create_or_update_with_create_or_update() {
        let t1 = AppTask::CreateOrUpdate {
            status_id: AppStatusChangeId::new(),
            app_name: AppName::master(),
            replicate_from: None,
            service_configs: vec![sc!("nginx", "nginx", env = ("NGINX_HOST" => "local.host"))],
            owners: vec![Owner {
                sub: SubjectIdentifier::new(String::from("github")),
                iss: IssuerUrl::new(String::from("https://github.com")).unwrap(),
                name: None,
            }],
            user_defined_parameters: Some(serde_json::json!({
                "string-key": "test",
                "array-key": [1, 2, 3]
            })),
        };
        let status_id_2 = AppStatusChangeId::new();
        let t2 = AppTask::CreateOrUpdate {
            status_id: status_id_2,
            app_name: AppName::master(),
            replicate_from: None,
            service_configs: vec![
                sc!("httpd"),
                sc!("nginx", "nginx", env = ("NGINX_HOST" => "my.host")),
            ],
            owners: vec![Owner {
                sub: SubjectIdentifier::new(String::from("gitlab")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: None,
            }],
            user_defined_parameters: Some(serde_json::json!({
                "string-key": "test-overwrite",
                "array-key": [4, 5, 6]
            })),
        };

        let merged = t1.merge_with(t2);

        assert_eq!(
            merged,
            AppTask::CreateOrUpdate {
                status_id: status_id_2,
                app_name: AppName::master(),
                replicate_from: None,
                service_configs: vec![
                    sc!("httpd"),
                    sc!("nginx", "nginx", env = ("NGINX_HOST" => "my.host")),
                ],
                owners: vec![
                    Owner {
                        sub: SubjectIdentifier::new(String::from("github")),
                        iss: IssuerUrl::new(String::from("https://github.com")).unwrap(),
                        name: None,
                    },
                    Owner {
                        sub: SubjectIdentifier::new(String::from("gitlab")),
                        iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                        name: None,
                    },
                ],
                user_defined_parameters: Some(serde_json::json!({
                    "string-key": "test-overwrite",
                    "array-key": [1, 2, 3, 4, 5, 6]
                })),
            },
        );
    }
}
