use openidconnect::{IssuerUrl, SubjectIdentifier};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Eq, PartialEq, Hash, Deserialize, Serialize)]
pub struct Owner {
    pub sub: SubjectIdentifier,
    pub iss: IssuerUrl,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Owner {
    pub fn normalize(owners: HashSet<Self>) -> HashSet<Self> {
        let mut map = HashMap::<(SubjectIdentifier, IssuerUrl), Option<String>>::new();

        for owner in owners.into_iter() {
            let Owner { sub, iss, mut name } = owner;

            map.entry((sub, iss))
                .and_modify(|existing_name| {
                    *existing_name = match (existing_name.take(), name.take()) {
                        (None, None) => None,
                        (None, Some(name)) => Some(name),
                        (Some(name), None) => Some(name),
                        (Some(name_1), Some(name_2)) => {
                            // names with spaces will be prioritize because they are most likely
                            // the real name.
                            match (name_1.contains(" "), name_2.contains(" ")) {
                                (true, false) => Some(name_1),
                                (false, true) => Some(name_2),
                                _ => {
                                    if name_1.len() > name_2.len() {
                                        Some(name_1)
                                    } else {
                                        Some(name_2)
                                    }
                                }
                            }
                        }
                    };
                })
                .or_insert(name);
        }

        map.into_iter()
            .map(|((sub, iss), name)| Owner { sub, iss, name })
            .collect::<HashSet<_>>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_owners_with_same_sub_issuer() {
        let owners = HashSet::from([
            Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: Some(String::from("user_login")),
            },
            Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: Some(String::from("Some Person")),
            },
            Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: None,
            },
        ]);

        let owners = Owner::normalize(owners);

        assert_eq!(
            owners,
            HashSet::from([Owner {
                sub: SubjectIdentifier::new(String::from("gitlab-user")),
                iss: IssuerUrl::new(String::from("https://gitlab.com")).unwrap(),
                name: Some(String::from("Some Person")),
            },])
        )
    }
}
