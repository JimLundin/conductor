use alloc::{borrow::ToOwned, format, string::String};

#[must_use]
pub fn display(remote: &str) -> String {
    match remote.split_once("://") {
        Some((scheme, address)) => {
            let (host, location) = authority(address);
            let path = public_path(location);
            format!("{scheme}://{host}{path}")
        }
        None => scp(remote).map_or_else(
            || remote.to_owned(),
            |(host, path)| format!("{host}:{path}"),
        ),
    }
}

fn authority(address: &str) -> (&str, &str) {
    let (login, path) = address
        .split_at(address.find(['/', '?', '#']).unwrap_or(address.len()));
    (without_user(login), path)
}

fn public_path(path: &str) -> &str {
    path.split(['?', '#']).next().unwrap_or_default()
}

fn without_user(login: &str) -> &str {
    login.rsplit_once('@').map_or(login, |(_, host)| host)
}

fn scp(remote: &str) -> Option<(&str, &str)> {
    let offset = remote
        .find('[')
        .filter(|at| *at == 0 || remote.as_bytes().get(at - 1) == Some(&b'@'))
        .and_then(|at| remote.get(at..)?.find(']').map(|end| at + end + 1))
        .unwrap_or(0);
    let separator = remote.get(offset..)?.find(':')? + offset;
    let (login, path_with_separator) = remote.split_at(separator);
    let path = path_with_separator.strip_prefix(':')?;
    if login.is_empty()
        || login.contains(['/', '\\'])
        || (login.len() == 1 && path.starts_with(['/', '\\']))
    {
        return None;
    }
    Some((without_user(login), path))
}

#[must_use]
pub fn normal(remote: &str) -> String {
    let network = match remote.split_once("://") {
        Some((scheme, address)) => {
            (!scheme.eq_ignore_ascii_case("file")).then(|| {
                let (host, path) = authority(address);
                (host, public_path(path))
            })
        }
        None => scp(remote),
    };
    network.map_or_else(
        || format!("local:{remote}"),
        |(host, path)| {
            let trimmed = path.trim_matches('/');
            let repository = trimmed.strip_suffix(".git").unwrap_or(trimmed);
            format!("network:{}/{repository}", host.to_ascii_lowercase())
        },
    )
}
