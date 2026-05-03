use crate::mock::ScenarioStep;

pub fn default_scenario() -> Vec<ScenarioStep> {
    vec![
        // Orchestrator receives the task
        ScenarioStep::CreateTask {
            id: "task-1".into(),
            title: "Build a REST API for user management".into(),
            assigned_to: "orchestrator".into(),
            delegated_by: None,
        },
        ScenarioStep::UpdateTaskState {
            task_id: "task-1".into(),
            new_state: crate::model::TaskState::Working,
        },
        ScenarioStep::Pause { ticks: 10 },

        // Orchestrator thinks and delegates schema design
        ScenarioStep::AgentThinks {
            agent: "orchestrator".into(),
            duration_ticks: 20,
        },
        ScenarioStep::SendMessage {
            from: "orchestrator".into(),
            to: "coder".into(),
            content: "I need you to design the database schema for a user management REST API. \
                      We need tables for users, roles, and permissions. Include proper foreign keys \
                      and indexes for common query patterns.".into(),
            chars_per_tick: 3,
        },
        ScenarioStep::DelegateTask {
            from: "orchestrator".into(),
            to: "coder".into(),
            parent_task_id: "task-1".into(),
            task_id: "task-1-1".into(),
            title: "Design database schema".into(),
        },
        ScenarioStep::Pause { ticks: 5 },

        // Coder thinks and responds with schema
        ScenarioStep::AgentThinks {
            agent: "coder".into(),
            duration_ticks: 25,
        },
        ScenarioStep::SendMessage {
            from: "coder".into(),
            to: "orchestrator".into(),
            content: "Here's the schema design:\n\n\
                      CREATE TABLE users (\n\
                      \x20 id UUID PRIMARY KEY,\n\
                      \x20 email VARCHAR(255) UNIQUE,\n\
                      \x20 name VARCHAR(100),\n\
                      \x20 created_at TIMESTAMP\n\
                      );\n\n\
                      CREATE TABLE roles (\n\
                      \x20 id SERIAL PRIMARY KEY,\n\
                      \x20 name VARCHAR(50) UNIQUE\n\
                      );\n\n\
                      CREATE TABLE user_roles (\n\
                      \x20 user_id UUID REFERENCES users(id),\n\
                      \x20 role_id INT REFERENCES roles(id),\n\
                      \x20 PRIMARY KEY (user_id, role_id)\n\
                      );".into(),
            chars_per_tick: 4,
        },
        ScenarioStep::UpdateTaskState {
            task_id: "task-1-1".into(),
            new_state: crate::model::TaskState::Completed,
        },
        ScenarioStep::Pause { ticks: 10 },

        // Orchestrator delegates review
        ScenarioStep::AgentThinks {
            agent: "orchestrator".into(),
            duration_ticks: 15,
        },
        ScenarioStep::SendMessage {
            from: "orchestrator".into(),
            to: "reviewer".into(),
            content: "Please review this database schema for the user management API. \
                      Check for normalization issues, missing indexes, and security concerns. \
                      The schema includes users, roles, and a junction table.".into(),
            chars_per_tick: 3,
        },
        ScenarioStep::DelegateTask {
            from: "orchestrator".into(),
            to: "reviewer".into(),
            parent_task_id: "task-1".into(),
            task_id: "task-1-2".into(),
            title: "Review database schema".into(),
        },
        ScenarioStep::Pause { ticks: 5 },

        // Reviewer thinks and provides feedback
        ScenarioStep::AgentThinks {
            agent: "reviewer".into(),
            duration_ticks: 30,
        },
        ScenarioStep::SendMessage {
            from: "reviewer".into(),
            to: "orchestrator".into(),
            content: "Schema review findings:\n\n\
                      1. ✓ Good use of UUID for user IDs\n\
                      2. ✓ Proper junction table for many-to-many\n\
                      3. ⚠ Missing: updated_at column on users\n\
                      4. ⚠ Missing: password_hash column\n\
                      5. ⚠ Add INDEX on user_roles(role_id)\n\
                      6. Consider: soft delete via deleted_at column\n\n\
                      Overall: solid foundation, needs the noted additions.".into(),
            chars_per_tick: 3,
        },
        ScenarioStep::UpdateTaskState {
            task_id: "task-1-2".into(),
            new_state: crate::model::TaskState::Completed,
        },
        ScenarioStep::Pause { ticks: 10 },

        // Orchestrator sends revision request to Coder
        ScenarioStep::AgentThinks {
            agent: "orchestrator".into(),
            duration_ticks: 10,
        },
        ScenarioStep::SendMessage {
            from: "orchestrator".into(),
            to: "coder".into(),
            content: "Good feedback from review. Please update the schema:\n\
                      - Add updated_at and password_hash to users\n\
                      - Add index on user_roles(role_id)\n\
                      - Add soft delete support via deleted_at".into(),
            chars_per_tick: 3,
        },
        ScenarioStep::DelegateTask {
            from: "orchestrator".into(),
            to: "coder".into(),
            parent_task_id: "task-1".into(),
            task_id: "task-1-3".into(),
            title: "Revise schema per review".into(),
        },
        ScenarioStep::Pause { ticks: 5 },

        // Coder revises
        ScenarioStep::AgentThinks {
            agent: "coder".into(),
            duration_ticks: 20,
        },
        ScenarioStep::SendMessage {
            from: "coder".into(),
            to: "orchestrator".into(),
            content: "Updated schema applied:\n\n\
                      ALTER TABLE users ADD COLUMN\n\
                      \x20 password_hash VARCHAR(255) NOT NULL,\n\
                      \x20 updated_at TIMESTAMP DEFAULT NOW(),\n\
                      \x20 deleted_at TIMESTAMP;\n\n\
                      CREATE INDEX idx_user_roles_role\n\
                      \x20 ON user_roles(role_id);\n\n\
                      All review items addressed. Ready for API layer.".into(),
            chars_per_tick: 4,
        },
        ScenarioStep::UpdateTaskState {
            task_id: "task-1-3".into(),
            new_state: crate::model::TaskState::Completed,
        },
        ScenarioStep::Pause { ticks: 10 },

        // Orchestrator wraps up
        ScenarioStep::AgentThinks {
            agent: "orchestrator".into(),
            duration_ticks: 10,
        },
        ScenarioStep::SendMessage {
            from: "orchestrator".into(),
            to: "coder".into(),
            content: "Schema looks great. Task complete. Moving on to API endpoint design.".into(),
            chars_per_tick: 3,
        },
        ScenarioStep::UpdateTaskState {
            task_id: "task-1".into(),
            new_state: crate::model::TaskState::Completed,
        },
    ]
}
