use crate::{
    core::config::Config,
    domain::models::identity::{Caller, IdentityError},
};
use std::net::IpAddr;
pub(crate) fn lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|ip| ip.is_loopback() || ip.is_private())
        }
    }
}
pub fn attribute(
    config: &Config,
    app_id: &str,
    user_id: Option<&str>,
    is_admin: bool,
    peer: Option<IpAddr>,
    require_user: bool,
) -> Result<Caller, IdentityError> {
    if !config.allowed_apps.contains(&app_id) {
        return Err(IdentityError::UnknownApp);
    }
    let user_id = user_id.filter(|s| !s.trim().is_empty());
    if user_id.is_none() && (require_user || !peer.is_some_and(lan)) {
        return Err(IdentityError::MissingUser);
    }
    Ok(Caller {
        app_id: app_id.into(),
        user_id: user_id.map(str::to_owned),
        is_admin,
    })
}
