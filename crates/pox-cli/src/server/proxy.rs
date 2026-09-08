//! Forwarding metadata is accepted only from explicitly configured proxy peers.
use hyper::{header, http::uri::Authority, HeaderMap, StatusCode};
use ipnet::IpNet;
use std::net::{IpAddr, SocketAddr};

pub(super) struct Identity {
    pub remote: SocketAddr,
    pub host: String,
    pub authority: Option<String>,
    pub port: u16,
    pub secure: bool,
}

fn single<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, StatusCode> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .map(|value| value.to_str().map_err(|_| StatusCode::BAD_REQUEST))
        .transpose()?;
    if values.next().is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(value)
}

fn authority(value: &str) -> Result<Authority, StatusCode> {
    let parsed = value
        .parse::<Authority>()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if parsed.host().is_empty()
        || value.contains(['@', ',', ' ', '\t'])
        || (value.len() > parsed.host().len() && parsed.port_u16().is_none())
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(parsed)
}

pub(super) fn resolve(
    headers: &HeaderMap,
    peer: SocketAddr,
    networks: &[IpNet],
    host: &str,
    port: u16,
) -> Result<Identity, StatusCode> {
    let mut identity = Identity {
        remote: peer,
        host: host.into(),
        authority: None,
        port,
        secure: false,
    };
    if let Some(value) = single(headers, header::HOST.as_str())? {
        identity.host = authority(value)?.host().trim_matches(['[', ']']).into();
    }
    let trusted = |address: IpAddr| {
        networks
            .iter()
            .any(|network| network.contains(&address.to_canonical()))
    };
    if !trusted(peer.ip()) {
        return Ok(identity);
    }
    // Walk from the connected peer towards the client, stopping at the first
    // untrusted address. A client-supplied prefix cannot replace that address.
    let mut chain = Vec::new();
    for value in headers.get_all("x-forwarded-for") {
        for address in value
            .to_str()
            .map_err(|_| StatusCode::BAD_REQUEST)?
            .split(',')
        {
            chain.push(
                address
                    .trim()
                    .parse::<IpAddr>()
                    .map_err(|_| StatusCode::BAD_REQUEST)?,
            );
        }
    }
    for address in chain.into_iter().rev() {
        if !trusted(identity.remote.ip()) {
            break;
        }
        identity.remote = SocketAddr::new(address.to_canonical(), 0);
    }
    let proto = single(headers, "x-forwarded-proto")?;
    let forwarded_host = single(headers, "x-forwarded-host")?;
    let forwarded_port = single(headers, "x-forwarded-port")?;
    if let Some(value) = proto {
        identity.secure = match value {
            "http" => false,
            "https" => true,
            _ => return Err(StatusCode::BAD_REQUEST),
        };
        identity.port = if identity.secure { 443 } else { 80 };
    }
    let public_host = forwarded_host.or(if proto.is_some() || forwarded_port.is_some() {
        single(headers, header::HOST.as_str())?
    } else {
        None
    });
    if let Some(value) = public_host {
        let parsed = authority(value)?;
        identity.host = parsed.host().trim_matches(['[', ']']).into();
        identity.port = parsed
            .port_u16()
            .unwrap_or(if identity.secure { 443 } else { 80 });
        identity.authority = Some(value.into());
    }
    if let Some(value) = forwarded_port {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(StatusCode::BAD_REQUEST);
        }
        let forwarded_port = value.parse::<u16>().map_err(|_| StatusCode::BAD_REQUEST)?;
        if forwarded_port == 0 {
            return Err(StatusCode::BAD_REQUEST);
        }
        if let Some(host) = &identity.authority {
            if authority(host)?
                .port_u16()
                .is_some_and(|port| port != forwarded_port)
            {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
        identity.port = forwarded_port;
        if let Some(host) = &mut identity.authority {
            if authority(host)?.port_u16().is_none()
                && forwarded_port != if identity.secure { 443 } else { 80 }
            {
                *host = format!("{host}:{forwarded_port}");
            }
        }
    }
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn walks_trusted_suffix_and_ignores_untrusted_peer_metadata() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "192.0.2.99, 198.51.100.7, 10.0.0.2".parse().unwrap(),
        );
        let networks = ["10.0.0.0/8".parse().unwrap()];
        let result = resolve(
            &headers,
            "10.0.0.1:12".parse().unwrap(),
            &networks,
            "local",
            8080,
        )
        .unwrap();
        assert_eq!(
            result.remote,
            "198.51.100.7:0".parse::<SocketAddr>().unwrap()
        );
        headers.insert("x-forwarded-proto", "not-a-scheme".parse().unwrap());
        let peer = "203.0.113.8:15".parse().unwrap();
        assert_eq!(
            resolve(&headers, peer, &networks, "local", 8080)
                .unwrap()
                .remote,
            peer
        );
    }
    #[test]
    fn ipv6_and_mapped_peers_are_matched_explicitly() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "2001:db8::7".parse().unwrap());
        let networks = ["127.0.0.1/32".parse().unwrap()];
        let result = resolve(
            &headers,
            "[::ffff:127.0.0.1]:12".parse().unwrap(),
            &networks,
            "local",
            8080,
        )
        .unwrap();
        assert_eq!(
            result.remote,
            "[2001:db8::7]:0".parse::<SocketAddr>().unwrap()
        );
    }
    #[test]
    fn public_host_and_port_are_consistent() {
        let networks = ["127.0.0.1/32".parse().unwrap()];
        let peer = "127.0.0.1:12".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("host", "public.example:8443".parse().unwrap());
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        assert_eq!(
            resolve(&headers, peer, &networks, "local", 8080)
                .unwrap()
                .port,
            8443
        );
        headers.insert("x-forwarded-host", "[2001:db8::1]".parse().unwrap());
        headers.insert("x-forwarded-port", "9443".parse().unwrap());
        let result = resolve(&headers, peer, &networks, "local", 8080).unwrap();
        assert_eq!(result.port, 9443);
        assert_eq!(result.authority.as_deref(), Some("[2001:db8::1]:9443"));
        headers.remove("x-forwarded-port");
        assert_eq!(
            resolve(&headers, peer, &networks, "local", 8080)
                .unwrap()
                .port,
            443
        );
    }
}
