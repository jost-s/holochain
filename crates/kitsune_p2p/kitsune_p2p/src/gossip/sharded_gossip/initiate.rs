use kitsune_p2p_types::dht::{arq::ArqSet, ArqBounds};
use rand::Rng;

use super::*;

impl ShardedGossipLocal {
    /// Try to initiate gossip if we don't currently
    /// have an outgoing gossip.
    pub(super) async fn try_initiate(
        &self,
        agent_info_session: &mut AgentInfoSession,
    ) -> KitsuneResult<Option<Outgoing>> {
        // Get local agents
        let (has_target, local_agents) = self.inner.share_mut(|i, _| {
            i.check_tgt_expired(self.gossip_type, self.tuning_params.gossip_round_timeout());
            let has_target = i.initiate_tgt.is_some();
            // Clear any expired rounds.
            i.round_map.current_rounds();
            Ok((has_target, i.local_agents.clone()))
        })?;
        // There's already a target so there's nothing to do.
        if has_target {
            return Ok(None);
        }

        // If we don't have a local agent then there's nothing to do.
        if local_agents.is_empty() {
            // No local agents so there's no one to initiate gossip from.
            return Ok(None);
        }

        // Get the local agents intervals.
        let intervals: Vec<ArqBounds> = agent_info_session
            .local_arqs()
            .into_iter()
            .map(|a| a.to_bounds_std())
            .collect();

        // Choose a remote agent to gossip with.
        let remote_agent = self
            .find_remote_agent_within_arcset(ArqSet::new(intervals.clone()), agent_info_session)
            .await?;

        let maybe_gossip = if let Some(next_target::Node {
            agent_info_list,
            cert,
            url,
        }) = remote_agent
        {
            let id = rand::thread_rng().gen();

            let gossip = ShardedGossipWire::initiate(
                intervals,
                id,
                agent_info_session.get_local_agents().to_vec(),
            );

            let tgt = ShardedGossipTarget {
                remote_agent_list: agent_info_list,
                cert: cert.clone(),
                tie_break: id,
                when_initiated: Some(Instant::now()),
                url: url.clone(),
            };

            self.inner.share_mut(|inner, _| {
                inner.initiate_tgt = Some(tgt);
                Ok(())
            })?;
            Some((cert, HowToConnect::Url(url.to_string()), gossip))
        } else {
            None
        };
        Ok(maybe_gossip)
    }

    /// Receiving an incoming initiate.
    /// - Send back the accept, agent bloom and ops bloom gossip messages.
    /// - Only send the agent bloom if this is a recent gossip type.
    pub(super) async fn incoming_initiate(
        &self,
        peer_cert: NodeCert,
        remote_arqs: Vec<ArqBounds>,
        remote_id: u32,
        remote_agent_list: Vec<AgentInfoSigned>,
        agent_info_session: &mut AgentInfoSession,
    ) -> KitsuneResult<Vec<ShardedGossipWire>> {
        let (local_agents, same_as_target, already_in_progress) =
            self.inner.share_mut(|i, _| {
                let already_in_progress = i.round_map.round_exists(&peer_cert);
                let same_as_target = i
                    .initiate_tgt
                    .as_ref()
                    .filter(|tgt| tgt.cert == peer_cert)
                    .map(|tgt| tgt.tie_break);
                Ok((i.local_agents.clone(), same_as_target, already_in_progress))
            })?;

        // The round is already in progress from our side.
        // The remote side should not be initiating.
        if already_in_progress {
            // This means one side has already started a round but
            // a stale initiate was received.
            return Ok(vec![ShardedGossipWire::already_in_progress()]);
        }

        // If this is the same connection as our current target then we need to decide who proceeds.
        if let Some(our_id) = same_as_target {
            // If we have a lower id then we proceed
            // and the remote will exit.
            // If we have a higher id than the remote
            // then we exit and the remote will proceed.
            // If we tie then we both exit (This will be very rare).
            if our_id >= remote_id {
                return Ok(Vec::with_capacity(0));
            } else {
                self.inner.share_mut(|i, _| {
                    i.initiate_tgt = None;
                    Ok(())
                })?;
            }
        }

        // If we don't have a local agent then there's nothing to do.
        if local_agents.is_empty() {
            // No local agents so there's no one to initiate gossip from.
            return Ok(vec![ShardedGossipWire::no_agents()]);
        }

        // Get the local intervals.
        let local_arqs: Vec<ArqBounds> = agent_info_session
            .local_arqs()
            .into_iter()
            .map(|arc| arc.to_bounds_std())
            .collect();

        let agent_list = agent_info_session.get_local_agents().to_vec();

        // Send the intervals back as the accept message.
        let mut gossip = vec![ShardedGossipWire::accept(local_arqs.clone(), agent_list)];

        // Generate the bloom filters and new state.
        let state = self
            .generate_blooms_or_regions(
                remote_agent_list.clone(),
                local_arqs,
                remote_arqs,
                &mut gossip,
                agent_info_session,
            )
            .await?;

        self.inner.share_mut(|inner, _| {
            // If this is not the target we are accepting
            // then record it as a remote round.
            if inner
                .initiate_tgt
                .as_ref()
                .map_or(true, |tgt| tgt.cert != peer_cert)
            {
                let mut metrics = inner.metrics.write();

                metrics.update_current_round(&peer_cert, self.gossip_type.into(), &state);
                metrics.record_accept(&remote_agent_list, self.gossip_type.into());
            }

            inner.round_map.insert(peer_cert.clone(), state);

            // If this is the target then we should clear the when initiated timeout.
            if let Some(tgt) = inner.initiate_tgt.as_mut() {
                if tgt.cert == peer_cert {
                    tgt.when_initiated = None;
                    // we also want to update the agent list
                    // with that reported by the remote end
                    tgt.remote_agent_list = remote_agent_list;
                }
            }
            Ok(())
        })?;
        Ok(gossip)
    }

    /// Fetch a current list of agents to initiate gossip with.
    #[cfg(test)]
    pub(super) async fn query_agents_by_local_agents(&self) -> KitsuneResult<Vec<AgentInfoSigned>> {
        let local_agents = self.inner.share_mut(|i, _| Ok(i.local_agents.clone()))?;

        Ok(store::all_agent_info(&self.host_api, &self.space)
            .await?
            .into_iter()
            .filter(|a| local_agents.contains(&a.agent))
            .collect())
    }

    /// Generate the bloom filters and generate a new state.
    /// - Agent bloom is only generated if this is a `Recent` gossip type.
    /// - Empty blooms are not created.
    /// - A new state is created for this round.
    pub(super) async fn generate_blooms_or_regions(
        &self,
        remote_agent_list: Vec<AgentInfoSigned>,
        local_arqs: Vec<ArqBounds>,
        remote_arqs: Vec<ArqBounds>,
        gossip: &mut Vec<ShardedGossipWire>,
        agent_info_session: &mut AgentInfoSession,
    ) -> KitsuneResult<RoundState> {
        let topo = self
            .host_api
            .get_topology(self.space.clone())
            .await
            .map_err(KitsuneError::other)?;

        // Create the common arc set from the remote and local arcs.
        let local_arqs = ArqSet::new(local_arqs);
        let remote_arqs = ArqSet::new(remote_arqs);
        let common_arqs = Arc::new(local_arqs.intersection(&topo, &remote_arqs));

        let region_set = if let GossipType::Historical = self.gossip_type {
            let region_set = store::query_region_set(
                self.host_api.clone().api,
                self.space.clone(),
                (*common_arqs).clone(),
            )
            .await?;
            gossip.push(ShardedGossipWire::op_regions(region_set.clone()));
            Some(region_set)
        } else {
            None
        };

        // Generate the new state.
        let mut state = self.new_state(
            remote_agent_list,
            common_arqs,
            region_set,
            self.tuning_params.gossip_round_timeout(),
        )?;

        // Generate the agent bloom.
        if let GossipType::Recent = self.gossip_type {
            let bloom = self
                .generate_agent_bloom(state.clone(), agent_info_session)
                .await?;
            if let Some(bloom) = bloom {
                let bloom = encode_bloom_filter(&bloom);
                gossip.push(ShardedGossipWire::agents(bloom));
            }

            // we consider recent gossip to have "sent its region"
            // for purposes of determining the round is complete
            state.regions_are_queued = true;

            self.next_bloom_batch(state, gossip).await
        } else {
            // Everything has already been taken care of for Historical
            // gossip already. Just mark this true so that the state will not
            // be considered "finished" until all op data is received.
            state.has_pending_historical_op_data = true;
            state.regions_are_queued = false;
            Ok(state)
        }
    }

    /// Generate the next batch of blooms from this state.
    /// If there is a saved cursor from a previous partial
    /// batch then this will pick up from there.
    /// Otherwise s batch of blooms for the entire search window
    /// will be attempted (if this is too many hashes then it will
    /// create a new partial batch of blooms.)
    pub(super) async fn next_bloom_batch(
        &self,
        mut state: RoundState,
        gossip: &mut Vec<ShardedGossipWire>,
    ) -> KitsuneResult<RoundState> {
        // Get the default window for this gossip loop.
        let mut window = self.calculate_time_range();

        // If there is a previously saved cursor then start from there.
        if let Some(cursor) = state.bloom_batch_cursor.take() {
            window.start = cursor;
        }
        let blooms = self
            .generate_op_blooms_for_time_window(&state.common_arq_set, window)
            .await?;

        let blooms = match blooms {
            bloom::Batch::Complete(blooms) => blooms,
            bloom::Batch::Partial { cursor, data } => {
                // This batch of blooms is partial so save the cursor in this rounds state.
                state.bloom_batch_cursor = Some(cursor);
                data
            }
        };

        // If no blooms were found for this time window then return a no overlap.
        if blooms.is_empty() {
            // Check if this is the final time window.
            gossip.push(ShardedGossipWire::op_bloom(
                EncodedTimedBloomFilter::NoOverlap,
                true,
            ));
        }

        let len = blooms.len();

        // Encode each bloom found for this time window.
        for (i, bloom) in blooms.into_iter().enumerate() {
            let time_window = bloom.time;
            let bloom = match bloom.bloom {
                // We have some hashes so request all missing from the bloom.
                Some(bloom) => {
                    let bytes = encode_bloom_filter(&bloom);
                    EncodedTimedBloomFilter::HaveHashes {
                        filter: bytes,
                        time_window,
                    }
                }
                // We have no hashes for this time window but we do have agents
                // that hold the arc so request all the ops the remote holds.
                None => EncodedTimedBloomFilter::MissingAllHashes { time_window },
            };
            state.increment_expected_op_blooms();

            // Check if this is the final time window and the final bloom for this window.
            if i == len - 1 && state.bloom_batch_cursor.is_none() {
                gossip.push(ShardedGossipWire::op_bloom(bloom, true));
            } else {
                gossip.push(ShardedGossipWire::op_bloom(bloom, false));
            }
        }

        Ok(state)
    }
}
