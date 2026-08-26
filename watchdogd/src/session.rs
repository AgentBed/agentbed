//! Session binding and authenticated envelope verification.

use crate::error::RpcError;
use crate::interfaces::{Entropy, PeerCred, PeerCredSource};
use crate::rpc::protocol::{SessionBind, SessionEstablished};
use crate::worker_group_tag::WorkerGroupTag;

#[derive(Debug, Clone)]
pub struct SessionState {
    pub(crate) established: Option<SessionEstablished>,
    pub(crate) last_counter: u64,
    #[allow(dead_code)]
    pub(crate) bound: Option<BoundSession>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundSession {
    pub host_id: String,
    pub tx_id: String,
    pub epoch: u64,
    pub lease_id: String,
    pub worker_group_tag: WorkerGroupTag,
}

impl SessionState {
    #[allow(clippy::similar_names)]
    pub(crate) fn try_bind(
        broker_uid: u32,
        broker_gid: u32,
        safe_mode: bool,
        durable_binding: &Option<BoundSession>,
        peer_cred: &dyn PeerCredSource,
        entropy: &dyn Entropy,
        bind: SessionBind,
    ) -> Result<(Self, SessionEstablished, Option<BoundSession>), RpcError> {
        let cred = peer_cred
            .peer_credentials()
            .map_err(|_| RpcError::WrongPeer)?;
        Self::try_bind_with_cred(
            broker_uid,
            broker_gid,
            safe_mode,
            durable_binding,
            &cred,
            entropy,
            bind,
        )
    }

    #[allow(clippy::similar_names, clippy::ref_option)]
    pub(crate) fn try_bind_with_cred(
        broker_uid: u32,
        broker_gid: u32,
        safe_mode: bool,
        durable_binding: &Option<BoundSession>,
        cred: &PeerCred,
        entropy: &dyn Entropy,
        bind: SessionBind,
    ) -> Result<(Self, SessionEstablished, Option<BoundSession>), RpcError> {
        if safe_mode {
            return Err(RpcError::SafeModeActive);
        }
        if cred.uid != broker_uid || cred.gid != broker_gid {
            return Err(RpcError::WrongPeer);
        }
        if let Some(existing) = durable_binding {
            if existing.tx_id != bind.tx_id
                || existing.epoch != bind.epoch
                || existing.lease_id != bind.lease_id
                || existing.worker_group_tag != bind.worker_group_tag
                || existing.host_id != bind.host_id
            {
                return Err(RpcError::StaleReconnect);
            }
        }
        let capability = derive_capability(entropy, cred);
        let established = SessionEstablished {
            capability,
            server_nonce: format!("srv-{}", bind.client_nonce),
            host_id: bind.host_id.clone(),
            tx_id: bind.tx_id.clone(),
            epoch: bind.epoch,
            counter: 0,
        };
        let bound = BoundSession {
            host_id: bind.host_id,
            tx_id: bind.tx_id,
            epoch: bind.epoch,
            lease_id: bind.lease_id,
            worker_group_tag: bind.worker_group_tag,
        };
        let state = Self {
            established: Some(established.clone()),
            last_counter: 0,
            bound: Some(bound.clone()),
        };
        Ok((state, established, Some(bound)))
    }

    #[allow(dead_code)]
    pub(crate) fn bound(&self) -> Option<&BoundSession> {
        self.bound.as_ref()
    }

    pub(crate) fn verify_authenticated(
        &mut self,
        capability: &[u8],
        counter: u64,
        host_id: &str,
        tx_id: &str,
        epoch: u64,
    ) -> Result<(), RpcError> {
        let established = self.established.as_ref().ok_or(RpcError::WrongCapability)?;
        if capability != established.capability {
            return Err(RpcError::WrongCapability);
        }
        if counter == 0 {
            return Err(RpcError::WrongCapability);
        }
        let expected = self
            .last_counter
            .checked_add(1)
            .ok_or(RpcError::WrongCapability)?;
        if counter != expected {
            if counter <= self.last_counter {
                return Err(RpcError::ReplayCounter);
            }
            return Err(RpcError::WrongCapability);
        }
        if host_id != established.host_id
            || tx_id != established.tx_id
            || epoch != established.epoch
        {
            return Err(RpcError::WrongBinding);
        }
        Ok(())
    }

    pub(crate) fn advance_counter(&mut self, counter: u64) {
        self.last_counter = counter;
    }
}

fn derive_capability(entropy: &dyn Entropy, cred: &PeerCred) -> Vec<u8> {
    let mut capability = vec![0u8; 32];
    entropy.fill(&mut capability);
    for (index, byte) in cred.pid.to_ne_bytes().into_iter().enumerate() {
        if let Some(slot) = capability.get_mut(index) {
            *slot ^= byte;
        }
    }
    for (index, byte) in cred.uid.to_ne_bytes().into_iter().enumerate() {
        if let Some(slot) = capability.get_mut(8 + index) {
            *slot ^= byte;
        }
    }
    for (index, byte) in cred.gid.to_ne_bytes().into_iter().enumerate() {
        if let Some(slot) = capability.get_mut(16 + index) {
            *slot ^= byte;
        }
    }
    capability
}
