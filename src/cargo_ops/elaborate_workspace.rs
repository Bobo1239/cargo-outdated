use std::io::{self, Write};
use std::collections::{HashMap, HashSet};

use cargo::core::{Dependency, Package, PackageId, Workspace};
use cargo::ops::{self, Packages};
use cargo::util::{CargoError, CargoErrorKind, CargoResult, Config};
use tabwriter::TabWriter;

use super::Options;
use super::pkg_status::*;

/// An elaborate workspace containing resolved dependencies and
/// the update status of packages
pub struct ElaborateWorkspace<'ela> {
    /// Package which is treated as the root of the dependency tree
    pub root: PackageId,
    pub workspace: &'ela Workspace<'ela>,
    pub pkgs: HashMap<PackageId, Package>,
    pub pkg_deps: HashMap<PackageId, HashMap<PackageId, Dependency>>,
    /// Map of package status
    ///
    /// Since the grandparent may specify desired features of parent,
    /// which influence the status of current, a tuple of
    /// `(grand, parent, current)` should be used as the unique id
    pub pkg_status: HashMap<(Option<PackageId>, Option<PackageId>, PackageId), PkgStatus>,
}

impl<'ela> ElaborateWorkspace<'ela> {
    /// Elaborate a `Workspace`
    pub fn from_workspace(
        workspace: &'ela Workspace,
        options: &Options,
    ) -> CargoResult<ElaborateWorkspace<'ela>> {
        let specs = Packages::All.into_package_id_specs(workspace)?;
        let (packages, resolve) = ops::resolve_ws_precisely(
            workspace,
            None,
            &options.flag_features,
            options.flag_all_features,
            options.flag_no_default_features,
            &specs,
        )?;
        let mut pkgs = HashMap::new();
        let mut pkg_deps = HashMap::new();
        for pkg_id in packages.package_ids() {
            let pkg = packages.get(pkg_id)?;
            pkgs.insert(pkg_id.clone(), pkg.clone());
            let deps = pkg.dependencies();
            let mut dep_map = HashMap::new();
            for dep_id in resolve.deps(pkg_id) {
                for d in deps {
                    if d.matches_id(dep_id) {
                        dep_map.insert(dep_id.clone(), d.clone());
                        break;
                    }
                }
            }
            pkg_deps.insert(pkg_id.clone(), dep_map);
        }

        let root = {
            let determine_root = || if let Some(ref root_name) = options.flag_root {
                let workspace_root = workspace.current()?;
                if root_name == workspace_root.name() {
                    Ok(workspace_root.package_id().clone())
                } else {
                    for direct_dep in pkg_deps[workspace_root.package_id()].keys() {
                        if pkgs[direct_dep].name() == root_name {
                            return Ok(direct_dep.clone());
                        }
                    }
                    return Err(CargoError::from_kind(CargoErrorKind::Msg(
                        "Root is neither the workspace root nor a direct dependency".to_owned(),
                    )));
                }
            } else {
                Ok(workspace.current()?.package_id().clone())
            };

            determine_root()?
        };

        Ok(ElaborateWorkspace {
            root: root,
            workspace: workspace,
            pkgs: pkgs,
            pkg_deps: pkg_deps,
            pkg_status: HashMap::new(),
        })
    }

    /// Resolve compatible and latest status from the corresponding `ElaborateWorkspace`s
    pub fn resolve_status(
        &mut self,
        compat: &ElaborateWorkspace,
        latest: &ElaborateWorkspace,
        options: &Options,
        config: &Config,
    ) -> CargoResult<()> {
        let root_parent = if &self.root == self.workspace.current()?.package_id() {
            None
        } else {
            Some(self.workspace.current()?.package_id())
        };
        let root_id = self.root.clone();
        self.resolve_status_recursive(
            None,
            root_parent,
            &root_id,
            Some(&compat.root),
            compat,
            Some(&latest.root),
            latest,
            options.flag_depth,
            config,
        )
    }

    #[allow(unknown_lints)]
    #[allow(too_many_arguments)]
    fn resolve_status_recursive(
        &mut self,
        grand: Option<&PackageId>,
        parent: Option<&PackageId>,
        self_pkg: &PackageId,
        compat_pkg: Option<&PackageId>,
        compat: &ElaborateWorkspace,
        latest_pkg: Option<&PackageId>,
        latest: &ElaborateWorkspace,
        depth: i32,
        config: &Config,
    ) -> CargoResult<()> {
        let pkg_status_key = (grand.cloned(), parent.cloned(), self_pkg.clone());
        if self.pkg_status.contains_key(&pkg_status_key) {
            return Ok(());
        }
        let self_pkg = self.pkgs.get(self_pkg).cloned().unwrap();
        let pkg_status = PkgStatus {
            compat: Status::from_versions(
                self_pkg.version(),
                compat_pkg
                    .and_then(|id| compat.pkgs.get(id))
                    .map(|p| p.version()),
            ),
            latest: Status::from_versions(
                self_pkg.version(),
                latest_pkg
                    .and_then(|id| latest.pkgs.get(id))
                    .map(|p| p.version()),
            ),
        };
        debug!(
            config,
            "UPDATE, self: {:?}, key: {:?}, status: {:?}\n",
            self_pkg.package_id(),
            pkg_status_key,
            pkg_status
        );
        self.pkg_status.insert(pkg_status_key, pkg_status);

        if depth == 0 {
            return Ok(());
        }

        debug!(
            config,
            "LOOP, parent: {:?}, self: {:?}, compat: {:?}, latest: {:?}\n",
            parent,
            self_pkg.package_id(),
            compat_pkg,
            latest_pkg
        );

        let self_deps: Vec<_> = self.pkg_deps[self_pkg.package_id()]
            .keys()
            .cloned()
            .collect();
        for next_self in self_deps {
            let next_name = self.pkgs[&next_self].name().to_owned();
            let next_compat = compat_pkg.and_then(|id| compat.pkg_deps.get(id)).and_then(
                |dep_map| {
                    for dep_id in dep_map.keys() {
                        let dep_name = compat.pkgs[dep_id].name();
                        if dep_name == next_name {
                            return Some(dep_id);
                        }
                    }
                    None
                },
            );
            let next_latest = latest_pkg.and_then(|id| latest.pkg_deps.get(id)).and_then(
                |dep_map| {
                    for dep_id in dep_map.keys() {
                        let dep_name = latest.pkgs[dep_id].name();
                        if dep_name == next_name {
                            return Some(dep_id);
                        }
                    }
                    None
                },
            );
            debug!(
                config,
                "NEXT, next_self: {:?}, next_compat: {:?}, next_latest: {:?}\n",
                next_self,
                next_compat,
                next_latest
            );
            self.resolve_status_recursive(
                parent,
                Some(self_pkg.package_id()),
                &next_self,
                next_compat,
                compat,
                next_latest,
                latest,
                depth - 1,
                config,
            )?;
        }

        Ok(())
    }

    /// Print package status to `TabWriter`
    pub fn print_list(&self, options: &Options, config: &Config) -> CargoResult<i32> {
        verbose!(config, "Printing...", "list format");
        let mut lines = vec![];
        let root_parent = if &self.root == self.workspace.current()?.package_id() {
            None
        } else {
            Some(self.workspace.current()?.package_id())
        };
        {
            let mut printed = HashSet::new();
            self.print_list_recursive(
                options,
                None,
                root_parent,
                &self.root,
                options.flag_depth,
                &mut lines,
                &mut printed,
            )?;
        }
        lines.sort();
        lines.dedup();
        let lines_len = lines.len();

        verbose!(config, "Printing...", "with tab writer");
        if lines.is_empty() {
            println!("All dependencies are up to date, yay!");
        } else {
            let mut tw = TabWriter::new(vec![]);
            write!(&mut tw, "Name\tProject\tCompat\tLatest\tKind\tPlatform\n")?;
            for line in lines {
                write!(&mut tw, "{}", line)?;
            }
            tw.flush()?;
            write!(
                io::stdout(),
                "{}",
                String::from_utf8(tw.into_inner().unwrap()).unwrap()
            )?;
            io::stdout().flush()?;
        }

        Ok(lines_len as i32)
    }

    #[allow(unknown_lints)]
    #[allow(too_many_arguments)]
    fn print_list_recursive(
        &self,
        options: &Options,
        grand: Option<&PackageId>,
        parent: Option<&PackageId>,
        pkg_id: &PackageId,
        depth: i32,
        lines: &mut Vec<String>,
        printed: &mut HashSet<(Option<PackageId>, Option<PackageId>, PackageId)>,
    ) -> CargoResult<()> {
        let pkg_status_key = (grand.cloned(), parent.cloned(), pkg_id.clone());
        if printed.contains(&pkg_status_key) {
            return Ok(());
        }
        printed.insert(pkg_status_key.clone());

        let pkg = &self.pkgs[pkg_id];
        let pkg_status = &self.pkg_status[&pkg_status_key];

        if (pkg_status.compat.is_changed() || pkg_status.latest.is_changed()) &&
            (options.flag_packages.is_empty() ||
                options.flag_packages.contains(&pkg.name().to_string()))
        {
            // name version compatible latest kind platform
            if let Some(parent) = parent {
                let dependency = &self.pkg_deps[parent][pkg_id];
                let label = if parent == self.workspace.current()?.package_id() {
                    pkg.name().to_owned()
                } else {
                    format!("{}->{}", self.pkgs[parent].name(), pkg.name())
                };
                let line = format!(
                    "{}\t{}\t{}\t{}\t{:?}\t{}\n",
                    label,
                    pkg.version(),
                    pkg_status.compat.to_string(),
                    pkg_status.latest.to_string(),
                    dependency.kind(),
                    dependency
                        .platform()
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "---".to_owned())
                );
                lines.push(line);
            } else {
                let line = format!(
                    "{}\t{}\t{}\t{}\t---\t---\n",
                    pkg.name(),
                    pkg.version(),
                    pkg_status.compat.to_string(),
                    pkg_status.latest.to_string()
                );
                lines.push(line);
            }
        }

        if depth == 0 {
            return Ok(());
        }

        for dep in self.pkg_deps[pkg_id].keys() {
            self.print_list_recursive(
                options,
                parent,
                Some(pkg_id),
                dep,
                depth - 1,
                lines,
                printed,
            )?;
        }

        Ok(())
    }
}
