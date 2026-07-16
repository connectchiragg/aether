use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AttributionCategory {
    UserPrompt,
    Context,
    Compaction,
    ToolsAndMcps,
    Hooks,
    Memory,
    DocumentsAndKbs,
    Agents,
    ProviderRuntime,
}

impl AttributionCategory {
    pub const ALL: [Self; 9] = [
        Self::UserPrompt,
        Self::Context,
        Self::Compaction,
        Self::ToolsAndMcps,
        Self::Hooks,
        Self::Memory,
        Self::DocumentsAndKbs,
        Self::Agents,
        Self::ProviderRuntime,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::UserPrompt => "User prompt",
            Self::Context => "Context",
            Self::Compaction => "Compaction",
            Self::ToolsAndMcps => "Tools & MCPs",
            Self::Hooks => "Hooks",
            Self::Memory => "Memory",
            Self::DocumentsAndKbs => "Documents & KBs",
            Self::Agents => "Agents",
            Self::ProviderRuntime => "Provider runtime",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AttributionNode {
    pub label: String,
    pub tokens: u64,
    pub percent_of_root: f64,
    pub estimated: bool,
    pub category: Option<AttributionCategory>,
    pub children: Vec<AttributionNode>,
}

impl AttributionNode {
    fn empty_root(label: impl Into<String>) -> Self {
        let mut children = AttributionCategory::ALL
            .into_iter()
            .map(|category| Self {
                label: category.label().to_string(),
                tokens: 0,
                percent_of_root: 0.0,
                estimated: true,
                category: Some(category),
                children: Vec::new(),
            })
            .collect::<Vec<_>>();
        sort_nodes(&mut children);
        Self {
            label: label.into(),
            tokens: 0,
            percent_of_root: 0.0,
            estimated: true,
            category: None,
            children,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RequestAttribution {
    pub id: String,
    pub actor: String,
    pub label: String,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub exact: bool,
    pub root: AttributionNode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AttributionObservation {
    category: AttributionCategory,
    source: String,
    invocation: String,
    estimated_tokens: u64,
}

#[derive(Clone, Debug)]
pub struct TurnAttribution {
    pub aggregate: AttributionNode,
    parent_requests: Vec<RequestAttribution>,
    agent_requests: BTreeMap<String, Vec<RequestAttribution>>,
    observations: Vec<AttributionObservation>,
    deferred_observations: Vec<AttributionObservation>,
    deferred_request_label: Option<String>,
    next_request_label: Option<String>,
    history_commit_floor: u64,
}

impl Default for TurnAttribution {
    fn default() -> Self {
        Self {
            aggregate: AttributionNode::empty_root("All work"),
            parent_requests: Vec::new(),
            agent_requests: BTreeMap::new(),
            observations: Vec::new(),
            deferred_observations: Vec::new(),
            deferred_request_label: None,
            next_request_label: Some("Initial request".to_string()),
            history_commit_floor: 0,
        }
    }
}

impl TurnAttribution {
    pub fn new(prompt: &str, prior_context_tokens: u64) -> Self {
        let mut attribution = Self::default();
        attribution.set_prompt(prompt);
        attribution.replace_context_estimate(prior_context_tokens);
        attribution.refresh_aggregate();
        attribution
    }

    pub fn set_prompt(&mut self, prompt: &str) {
        self.observations
            .retain(|item| item.category != AttributionCategory::UserPrompt);
        let tokens = estimate_tokens(prompt);
        if tokens > 0 {
            self.observe(
                AttributionCategory::UserPrompt,
                "Current prompt",
                instruction_preview(prompt),
                tokens,
            );
        }
    }

    pub fn observe(
        &mut self,
        category: AttributionCategory,
        source: impl Into<String>,
        invocation: impl Into<String>,
        estimated_tokens: u64,
    ) {
        if estimated_tokens == 0 {
            return;
        }
        merge_observation(
            &mut self.observations,
            AttributionObservation {
                category,
                source: source.into(),
                invocation: invocation.into(),
                estimated_tokens,
            },
        );
    }

    /// Queue model output that becomes input only after the current request finishes.
    pub fn defer_after_request(
        &mut self,
        category: AttributionCategory,
        source: impl Into<String>,
        invocation: impl Into<String>,
        estimated_tokens: u64,
        next_request_label: Option<String>,
    ) {
        if estimated_tokens > 0 {
            merge_observation(
                &mut self.deferred_observations,
                AttributionObservation {
                    category,
                    source: source.into(),
                    invocation: invocation.into(),
                    estimated_tokens,
                },
            );
        }
        if next_request_label.is_some() {
            self.deferred_request_label = next_request_label;
        }
    }

    pub fn flush_deferred(&mut self) {
        for observation in std::mem::take(&mut self.deferred_observations) {
            merge_observation(&mut self.observations, observation);
        }
        if let Some(label) = self.deferred_request_label.take() {
            self.next_request_label = Some(label);
        }
    }

    pub fn set_next_request_label(&mut self, label: impl Into<String>) {
        self.next_request_label = Some(label.into());
    }

    pub fn record_parent_request(
        &mut self,
        id: impl Into<String>,
        input_tokens: u64,
        cached_input_tokens: u64,
        exact: bool,
    ) -> u64 {
        let request_number = self.parent_requests.len() + 1;
        let label = self
            .next_request_label
            .take()
            .unwrap_or_else(|| format!("Request {request_number}"));
        let request = build_request(
            id.into(),
            "Parent".to_string(),
            label,
            input_tokens,
            cached_input_tokens,
            exact,
            &self.observations,
        );
        let context_tokens =
            category_tokens(&request.root, AttributionCategory::Context).saturating_add(
                category_tokens(&request.root, AttributionCategory::Compaction),
            );
        self.parent_requests.push(request);
        self.flush_deferred();
        self.refresh_aggregate();
        context_tokens
    }

    pub fn record_request_for_actor(
        &mut self,
        id: impl Into<String>,
        actor: impl Into<String>,
        input_tokens: u64,
        cached_input_tokens: u64,
        exact: bool,
    ) -> RequestAttribution {
        let request_number = self.parent_requests.len() + 1;
        let label = self
            .next_request_label
            .take()
            .unwrap_or_else(|| format!("Request {request_number}"));
        let actor = actor.into();
        let agent_observations = actor
            .strip_prefix("Agent: ")
            .map(|agent| observations_for_agent(agent, &self.observations));
        let observations = agent_observations.as_deref().unwrap_or(&self.observations);
        let request = build_request(
            id.into(),
            actor,
            label,
            input_tokens,
            cached_input_tokens,
            exact,
            observations,
        );
        self.parent_requests.push(request.clone());
        self.flush_deferred();
        self.refresh_aggregate();
        request
    }

    pub fn set_agent_requests(
        &mut self,
        agent_id: impl Into<String>,
        requests: Vec<RequestAttribution>,
    ) {
        self.agent_requests.insert(agent_id.into(), requests);
        self.refresh_aggregate();
    }

    pub fn request_count(&self) -> usize {
        self.parent_requests.len() + self.agent_requests.values().map(Vec::len).sum::<usize>()
    }

    pub fn request(&self, index: usize) -> Option<&RequestAttribution> {
        if index < self.parent_requests.len() {
            return self.parent_requests.get(index);
        }
        let mut remaining = index - self.parent_requests.len();
        for requests in self.agent_requests.values() {
            if remaining < requests.len() {
                return requests.get(remaining);
            }
            remaining -= requests.len();
        }
        None
    }

    pub fn replace_context_estimate(&mut self, tokens: u64) {
        self.observations.retain(|item| {
            !matches!(
                item.category,
                AttributionCategory::Context | AttributionCategory::Compaction
            )
        });
        if tokens > 0 {
            self.observe(
                AttributionCategory::Context,
                "Prior turns",
                "Active history",
                tokens,
            );
        }
    }

    pub fn replace_active_history_estimate(
        &mut self,
        total_tokens: u64,
        compacted_tokens: u64,
        compaction_source: &str,
    ) {
        self.observations.retain(|item| {
            !matches!(
                item.category,
                AttributionCategory::Context | AttributionCategory::Compaction
            )
        });
        let compacted_tokens = compacted_tokens.min(total_tokens);
        let context_tokens = total_tokens.saturating_sub(compacted_tokens);
        if context_tokens > 0 {
            self.observe(
                AttributionCategory::Context,
                "Prior turns",
                "Active history",
                context_tokens,
            );
        }
        if compacted_tokens > 0 {
            self.observe(
                AttributionCategory::Compaction,
                compaction_source,
                "Active summary",
                compacted_tokens,
            );
        }
    }

    pub fn mark_compaction(&mut self) {
        self.flush_deferred();
        self.history_commit_floor = self.history_delta_estimate();
    }

    pub fn uncommitted_history_delta(&self) -> u64 {
        self.history_delta_estimate()
            .saturating_sub(self.history_commit_floor)
    }

    pub fn history_delta_estimate(&self) -> u64 {
        self.observations
            .iter()
            .filter(|item| {
                !matches!(
                    item.category,
                    AttributionCategory::Context
                        | AttributionCategory::Compaction
                        | AttributionCategory::ProviderRuntime
                )
            })
            .map(|item| item.estimated_tokens)
            .sum()
    }

    fn refresh_aggregate(&mut self) {
        let requests: Vec<&RequestAttribution> = self
            .parent_requests
            .iter()
            .chain(self.agent_requests.values().flatten())
            .collect();
        if requests.is_empty() {
            self.aggregate = build_root("All work", 0, 0, false, &self.observations);
            return;
        }

        let total_tokens = requests.iter().map(|request| request.input_tokens).sum();
        let cached_tokens = requests
            .iter()
            .map(|request| request.cached_input_tokens)
            .sum();
        let exact = requests.iter().all(|request| request.exact);
        let mut observations = Vec::new();
        for request in requests {
            append_leaf_observations(&request.root, &mut observations);
        }
        self.aggregate = build_root(
            "All work",
            total_tokens,
            cached_tokens,
            exact,
            &observations,
        );
    }
}

pub fn estimate_tokens(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    let units = text
        .chars()
        .map(|character| if character.is_ascii() { 1_u64 } else { 3_u64 })
        .sum::<u64>();
    units.div_ceil(4).max(1)
}

fn merge_observation(
    observations: &mut Vec<AttributionObservation>,
    incoming: AttributionObservation,
) {
    if let Some(existing) = observations.iter_mut().find(|item| {
        item.category == incoming.category
            && item.source == incoming.source
            && item.invocation == incoming.invocation
    }) {
        existing.estimated_tokens = existing
            .estimated_tokens
            .saturating_add(incoming.estimated_tokens);
    } else {
        observations.push(incoming);
    }
}

fn observations_for_agent(
    agent: &str,
    observations: &[AttributionObservation],
) -> Vec<AttributionObservation> {
    observations
        .iter()
        .cloned()
        .map(|mut observation| {
            if observation.category == AttributionCategory::UserPrompt {
                observation.category = AttributionCategory::Agents;
                observation.source = agent.to_string();
            }
            observation
        })
        .collect()
}

fn instruction_preview(prompt: &str) -> String {
    const MAX_CHARS: usize = 56;
    let mut normalized = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = normalized.to_ascii_lowercase();
    if (lower.starts_with("for a ") || lower.starts_with("for an "))
        && lower
            .find(',')
            .is_some_and(|comma| comma <= 48 && lower[..comma].contains(" demo"))
    {
        normalized = normalized
            .split_once(',')
            .map(|(_, instruction)| instruction.trim_start().to_string())
            .unwrap_or(normalized);
    }
    if normalized
        .to_ascii_lowercase()
        .ends_with(" do not use tools.")
    {
        normalized.truncate(normalized.len() - " Do not use tools.".len());
    }

    let mut preview = normalized.chars().take(MAX_CHARS).collect::<String>();
    if normalized.chars().count() > MAX_CHARS {
        preview.truncate(
            preview
                .char_indices()
                .nth(MAX_CHARS.saturating_sub(3))
                .map(|(index, _)| index)
                .unwrap_or(preview.len()),
        );
        preview.push_str("...");
    }
    if preview.is_empty() {
        "Instruction".to_string()
    } else {
        preview
    }
}

fn build_request(
    id: String,
    actor: String,
    label: String,
    input_tokens: u64,
    cached_input_tokens: u64,
    exact: bool,
    observations: &[AttributionObservation],
) -> RequestAttribution {
    let root = build_root(
        label.clone(),
        input_tokens,
        cached_input_tokens,
        exact,
        observations,
    );
    RequestAttribution {
        id,
        actor,
        label,
        input_tokens: root.tokens,
        cached_input_tokens,
        exact,
        root,
    }
}

fn build_root(
    label: impl Into<String>,
    reported_tokens: u64,
    _cached_input_tokens: u64,
    exact: bool,
    observations: &[AttributionObservation],
) -> AttributionNode {
    let mut reconciled = observations.to_vec();
    let estimated_total = reconciled
        .iter()
        .map(|item| item.estimated_tokens)
        .sum::<u64>();
    let root_tokens = if exact || reported_tokens > 0 {
        reported_tokens
    } else {
        estimated_total
    };

    if root_tokens == 0 {
        let mut root = AttributionNode::empty_root(label);
        root.estimated = !exact;
        return root;
    }

    if estimated_total < root_tokens {
        merge_observation(
            &mut reconciled,
            AttributionObservation {
                category: AttributionCategory::ProviderRuntime,
                source: "Provider-managed input".to_string(),
                invocation: "Runtime and formatting".to_string(),
                estimated_tokens: root_tokens - estimated_total,
            },
        );
    } else if estimated_total > root_tokens {
        scale_observations(&mut reconciled, root_tokens, estimated_total);
    }

    let mut grouped: BTreeMap<AttributionCategory, BTreeMap<String, BTreeMap<String, u64>>> =
        BTreeMap::new();
    for observation in reconciled {
        if observation.estimated_tokens == 0 {
            continue;
        }
        *grouped
            .entry(observation.category)
            .or_default()
            .entry(observation.source)
            .or_default()
            .entry(observation.invocation)
            .or_default() += observation.estimated_tokens;
    }

    let mut children = AttributionCategory::ALL
        .into_iter()
        .map(|category| {
            let mut sources = grouped
                .remove(&category)
                .unwrap_or_default()
                .into_iter()
                .map(|(source, invocations)| {
                    let mut invocation_nodes = invocations
                        .into_iter()
                        .map(|(invocation, tokens)| AttributionNode {
                            label: invocation,
                            tokens,
                            percent_of_root: percentage(tokens, root_tokens),
                            estimated: true,
                            category: Some(category),
                            children: Vec::new(),
                        })
                        .collect::<Vec<_>>();
                    sort_nodes(&mut invocation_nodes);
                    let tokens = invocation_nodes.iter().map(|node| node.tokens).sum();
                    AttributionNode {
                        label: source,
                        tokens,
                        percent_of_root: percentage(tokens, root_tokens),
                        estimated: true,
                        category: Some(category),
                        children: invocation_nodes,
                    }
                })
                .collect::<Vec<_>>();
            sort_nodes(&mut sources);
            let tokens = sources.iter().map(|node| node.tokens).sum();
            AttributionNode {
                label: category.label().to_string(),
                tokens,
                percent_of_root: percentage(tokens, root_tokens),
                estimated: true,
                category: Some(category),
                children: sources,
            }
        })
        .collect::<Vec<_>>();
    sort_nodes(&mut children);

    AttributionNode {
        label: label.into(),
        tokens: root_tokens,
        percent_of_root: 100.0,
        estimated: !exact,
        category: None,
        children,
    }
}

fn scale_observations(observations: &mut [AttributionObservation], target: u64, source_total: u64) {
    let mut assigned = 0_u64;
    let mut remainders = Vec::with_capacity(observations.len());
    for (index, observation) in observations.iter_mut().enumerate() {
        let product = observation.estimated_tokens as u128 * target as u128;
        let scaled = (product / source_total as u128) as u64;
        let remainder = (product % source_total as u128) as u64;
        observation.estimated_tokens = scaled;
        assigned = assigned.saturating_add(scaled);
        remainders.push((remainder, index));
    }
    remainders.sort_by(|left, right| right.cmp(left));
    for (_, index) in remainders
        .into_iter()
        .take(target.saturating_sub(assigned) as usize)
    {
        observations[index].estimated_tokens += 1;
    }
}

fn append_leaf_observations(root: &AttributionNode, output: &mut Vec<AttributionObservation>) {
    for category in &root.children {
        let Some(category_kind) = category.category else {
            continue;
        };
        for source in &category.children {
            for invocation in &source.children {
                merge_observation(
                    output,
                    AttributionObservation {
                        category: category_kind,
                        source: source.label.clone(),
                        invocation: invocation.label.clone(),
                        estimated_tokens: invocation.tokens,
                    },
                );
            }
        }
    }
}

fn category_tokens(root: &AttributionNode, category: AttributionCategory) -> u64 {
    root.children
        .iter()
        .find(|node| node.category == Some(category))
        .map(|node| node.tokens)
        .unwrap_or(0)
}

fn percentage(tokens: u64, root_tokens: u64) -> f64 {
    tokens as f64 / root_tokens.max(1) as f64 * 100.0
}

fn sort_nodes(nodes: &mut [AttributionNode]) {
    nodes.sort_by(|left, right| {
        right
            .tokens
            .cmp(&left.tokens)
            .then_with(|| left.label.cmp(&right.label))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_categories_reconcile_to_exact_native_total() {
        let mut attribution = TurnAttribution::new("hello", 100);
        attribution.observe(
            AttributionCategory::ToolsAndMcps,
            "Read",
            "Read README.md #1",
            20,
        );
        attribution.record_parent_request("request-1", 500, 300, true);

        assert_eq!(attribution.aggregate.tokens, 500);
        assert!(!attribution.aggregate.estimated);
        assert_eq!(
            attribution
                .aggregate
                .children
                .iter()
                .map(|node| node.tokens)
                .sum::<u64>(),
            500
        );
        assert_eq!(
            category_tokens(&attribution.aggregate, AttributionCategory::ProviderRuntime),
            378
        );
    }

    #[test]
    fn all_work_counts_repeated_context_in_each_request() {
        let mut attribution = TurnAttribution::new("hello", 10_000);
        attribution.record_parent_request("request-1", 11_000, 0, true);
        attribution.set_next_request_label("After cargo test");
        attribution.record_parent_request("request-2", 12_000, 0, true);

        assert_eq!(attribution.request_count(), 2);
        assert_eq!(attribution.aggregate.tokens, 23_000);
        assert_eq!(attribution.request(1).unwrap().label, "After cargo test");
        assert!(category_tokens(&attribution.aggregate, AttributionCategory::Context) >= 20_000);
    }

    #[test]
    fn observations_are_aggregated_at_request_boundaries() {
        let mut attribution = TurnAttribution::new("hello", 0);
        attribution.record_parent_request("request-1", 100, 0, true);
        attribution.observe(
            AttributionCategory::ToolsAndMcps,
            "Read",
            "Read README.md #1",
            20,
        );

        assert_eq!(
            category_tokens(&attribution.aggregate, AttributionCategory::ToolsAndMcps),
            0
        );

        attribution.record_parent_request("request-2", 150, 0, true);

        assert!(category_tokens(&attribution.aggregate, AttributionCategory::ToolsAndMcps) > 0);
    }

    #[test]
    fn agent_request_attributes_its_instruction_to_the_named_agent() {
        let mut attribution = TurnAttribution::new("research the parser", 0);
        let request =
            attribution.record_request_for_actor("agent-request-1", "Agent: Godel", 100, 0, true);

        assert_eq!(
            category_tokens(&request.root, AttributionCategory::UserPrompt),
            0
        );
        let agents = request
            .root
            .children
            .iter()
            .find(|node| node.category == Some(AttributionCategory::Agents))
            .expect("agents category");
        let source = agents
            .children
            .iter()
            .find(|source| source.label == "Godel")
            .expect("named agent source");
        assert!(source
            .children
            .iter()
            .any(|invocation| invocation.label == "research the parser"));
    }

    #[test]
    fn agent_instruction_preview_removes_demo_boilerplate_and_truncates() {
        let preview = instruction_preview(
            "For an observability demo, reply with only the number of vowels in the word observability. Do not use tools.",
        );

        assert!(preview.starts_with("reply with only the number of vowels"));
        assert!(!preview.contains("observability demo"));
        assert!(preview.ends_with("..."));
        assert!(preview.chars().count() <= 56);
    }

    #[test]
    fn overestimated_children_scale_deterministically() {
        let mut attribution = TurnAttribution::new("12345678", 100);
        attribution.observe(
            AttributionCategory::ToolsAndMcps,
            "Read",
            "Read file #1",
            100,
        );
        attribution.record_parent_request("request-1", 10, 0, true);

        assert_eq!(
            attribution
                .aggregate
                .children
                .iter()
                .map(|node| node.tokens)
                .sum::<u64>(),
            10
        );
    }

    #[test]
    fn compaction_excludes_pre_compaction_turn_delta_from_next_commit() {
        let mut attribution = TurnAttribution::new("prompt", 1_000);
        attribution.observe(
            AttributionCategory::ToolsAndMcps,
            "Read",
            "Read before #1",
            100,
        );
        attribution.mark_compaction();
        attribution.observe(
            AttributionCategory::ToolsAndMcps,
            "Bash",
            "cargo test #2",
            50,
        );

        assert_eq!(attribution.uncommitted_history_delta(), 50);
    }

    #[test]
    fn compacted_summary_and_post_compaction_context_stay_separate() {
        let mut attribution = TurnAttribution::new("continue", 0);
        attribution.replace_active_history_estimate(1_400, 400, "Manual compaction");
        attribution.record_parent_request("request-1", 2_000, 0, true);

        let request = &attribution.request(0).unwrap().root;
        assert_eq!(
            category_tokens(request, AttributionCategory::Compaction),
            400
        );
        assert_eq!(
            category_tokens(request, AttributionCategory::Context),
            1_000
        );
        let compaction = request
            .children
            .iter()
            .find(|node| node.category == Some(AttributionCategory::Compaction))
            .expect("compaction category");
        assert_eq!(compaction.children[0].label, "Manual compaction");
    }

    #[test]
    fn every_attribution_level_sorts_by_tokens_then_label() {
        let mut attribution = TurnAttribution::new("sort this tree", 100);
        for (source, invocation, tokens) in [
            ("Terminal", "cargo test", 200),
            ("Terminal", "cargo fmt", 50),
            ("Read", "README.md", 125),
        ] {
            attribution.observe(
                AttributionCategory::ToolsAndMcps,
                source,
                invocation,
                tokens,
            );
        }
        attribution.observe(
            AttributionCategory::DocumentsAndKbs,
            "design.png",
            "Attached to prompt",
            300,
        );
        attribution.record_parent_request("request-1", 1_000, 0, true);

        fn assert_sorted(node: &AttributionNode) {
            assert!(node.children.windows(2).all(|pair| {
                pair[0].tokens > pair[1].tokens
                    || pair[0].tokens == pair[1].tokens && pair[0].label <= pair[1].label
            }));
            for child in &node.children {
                assert_sorted(child);
            }
        }

        assert_sorted(&attribution.aggregate);
    }
}
