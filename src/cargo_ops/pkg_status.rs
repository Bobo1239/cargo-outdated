use semver::Version;

#[derive(Debug)]
pub enum Status {
    Unchanged,
    Removed,
    Version(Version),
}

impl Status {
    pub fn from_versions(from: &Version, to: Option<&Version>) -> Status {
        if let Some(to) = to {
            if from == to {
                Status::Unchanged
            } else {
                Status::Version(to.clone())
            }
        } else {
            Status::Removed
        }
    }

    pub fn is_changed(&self) -> bool {
        match *self {
            Status::Unchanged => false,
            _ => true,
        }
    }
}

impl ::std::string::ToString for Status {
    fn to_string(&self) -> String {
        match *self {
            Status::Unchanged => "---".to_owned(),
            Status::Removed => "Removed".to_owned(),
            Status::Version(ref v) => v.to_string(),
        }
    }
}

#[derive(Debug)]
pub struct PkgStatus {
    pub compat: Status,
    pub latest: Status,
}
