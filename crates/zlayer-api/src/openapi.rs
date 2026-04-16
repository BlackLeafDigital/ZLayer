//! `OpenAPI` documentation generation

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

// Import types for schema definitions
use crate::handlers::auth::{
    BootstrapRequest, CsrfResponse, LoginRequest, LoginResponse, TokenRequest, TokenResponse,
    UserView,
};
use crate::handlers::build::{
    BuildRequest, BuildRequestWithContext, BuildStateEnum, BuildStatus, TemplateInfo,
    TriggerBuildResponse,
};
use crate::handlers::credentials::{
    CreateGitCredentialRequest, CreateRegistryCredentialRequest, GitCredentialKindSchema,
    GitCredentialResponse, RegistryAuthTypeSchema, RegistryCredentialResponse,
};
use crate::handlers::deployments::{CreateDeploymentRequest, DeploymentDetails, DeploymentSummary};
use crate::handlers::environments::{CreateEnvironmentRequest, UpdateEnvironmentRequest};
use crate::handlers::health::HealthResponse;
use crate::handlers::projects::{
    CreateProjectRequest, LinkDeploymentRequest, ProjectPullResponse, UpdateProjectRequest,
};
use crate::handlers::users::{CreateUserRequest, SetPasswordRequest, UpdateUserRequest};
use crate::handlers::webhooks::{WebhookInfoResponse, WebhookResponse};
// Note: Node and overlay types are not yet exposed in OpenAPI paths
// They will be uncommented when the corresponding endpoints are added
// use crate::handlers::nodes::{
//     JoinTokenResponse, NodeDetails, NodeResourceInfo, NodeSummary, UpdateLabelsRequest,
//     UpdateLabelsResponse,
// };
// use crate::handlers::overlay::{
//     DnsStatusResponse, IpAllocationResponse, OverlayStatusResponse, PeerInfo, PeerListResponse,
// };
use crate::handlers::containers::{
    ContainerExecRequest, ContainerExecResponse, ContainerInfo, ContainerResourceLimits,
    ContainerStatsResponse, ContainerWaitResponse, CreateContainerRequest, VolumeMount,
};
use crate::handlers::groups::{
    AddMemberRequest, CreateGroupRequest, GroupMembersResponse, UpdateGroupRequest,
};
use crate::handlers::notifiers::{
    CreateNotifierRequest, TestNotifierResponse, UpdateNotifierRequest,
};
use crate::handlers::secrets::{BulkImportResponse, CreateSecretRequest, SecretMetadataResponse};
use crate::handlers::services::{
    ScaleRequest, ServiceDetails, ServiceEndpoint, ServiceMetrics, ServiceSummary,
};
use crate::handlers::syncs::{
    CreateSyncRequest, SyncApplyResponse, SyncDiffResponse, SyncResourceResponse,
    SyncResourceResult,
};
use crate::handlers::tasks::CreateTaskRequest;
use crate::handlers::variables::{CreateVariableRequest, UpdateVariableRequest};
use crate::handlers::workflows::CreateWorkflowRequest;

// Internal API types
use crate::handlers::internal::{InternalScaleRequest, InternalScaleResponse};

use crate::handlers::permissions::GrantPermissionRequest;

// Import the auto-generated path types from utoipa macros
use crate::handlers::audit::__path_list_audit;
use crate::handlers::auth::{
    __path_bootstrap, __path_csrf, __path_get_token, __path_login, __path_logout, __path_me,
};
use crate::handlers::build::{
    __path_get_build_logs, __path_get_build_status, __path_list_builds,
    __path_list_runtime_templates, __path_start_build, __path_start_build_json,
    __path_stream_build,
};
use crate::handlers::containers::{
    __path_create_container, __path_delete_container, __path_exec_in_container,
    __path_get_container, __path_get_container_logs, __path_get_container_stats,
    __path_list_containers, __path_wait_container,
};
use crate::handlers::credentials::{
    __path_create_git_credential, __path_create_registry_credential, __path_delete_git_credential,
    __path_delete_registry_credential, __path_list_git_credentials,
    __path_list_registry_credentials,
};
use crate::handlers::deployments::{
    __path_create_deployment, __path_delete_deployment, __path_get_deployment,
    __path_list_deployments,
};
use crate::handlers::environments::{
    __path_create_environment, __path_delete_environment, __path_get_environment,
    __path_list_environments, __path_update_environment,
};
use crate::handlers::groups::{
    __path_add_member, __path_create_group, __path_delete_group, __path_get_group,
    __path_list_groups, __path_remove_member, __path_update_group,
};
use crate::handlers::health::{__path_liveness, __path_readiness};
use crate::handlers::internal::{__path_get_replicas_internal, __path_scale_service_internal};
use crate::handlers::permissions::{
    __path_grant_permission, __path_list_permissions, __path_revoke_permission,
};
use crate::handlers::users::{
    __path_create_user, __path_delete_user, __path_get_user, __path_list_users,
    __path_set_password, __path_update_user,
};
// Note: Node and overlay paths are not yet exposed in OpenAPI
// They will be uncommented when the corresponding endpoints are added
// use crate::handlers::nodes::{
//     __path_generate_join_token, __path_get_node, __path_list_nodes, __path_update_node_labels,
// };
// use crate::handlers::overlay::{
//     __path_get_dns_status, __path_get_ip_allocation, __path_get_overlay_peers,
//     __path_get_overlay_status,
// };
use crate::handlers::notifiers::{
    __path_create_notifier, __path_delete_notifier, __path_get_notifier, __path_list_notifiers,
    __path_test_notifier, __path_update_notifier,
};
use crate::handlers::projects::{
    __path_create_project, __path_delete_project, __path_get_project,
    __path_link_project_deployment, __path_list_project_deployments, __path_list_projects,
    __path_pull_project, __path_unlink_project_deployment, __path_update_project,
};
use crate::handlers::secrets::{
    __path_bulk_import_secrets, __path_create_secret, __path_delete_secret,
    __path_get_secret_metadata, __path_list_secrets,
};
use crate::handlers::services::{
    __path_get_service, __path_get_service_logs, __path_list_services, __path_scale_service,
};
use crate::handlers::syncs::{
    __path_apply_sync, __path_create_sync, __path_delete_sync, __path_diff_sync, __path_list_syncs,
};
use crate::handlers::tasks::{
    __path_create_task, __path_delete_task, __path_get_task, __path_list_task_runs,
    __path_list_tasks, __path_run_task,
};
use crate::handlers::variables::{
    __path_create_variable, __path_delete_variable, __path_get_variable, __path_list_variables,
    __path_update_variable,
};
use crate::handlers::webhooks::{
    __path_get_webhook_info, __path_receive_webhook, __path_rotate_webhook_secret,
};
use crate::handlers::workflows::{
    __path_create_workflow, __path_delete_workflow, __path_get_workflow, __path_list_workflow_runs,
    __path_list_workflows, __path_run_workflow,
};

/// Security addon for adding Bearer JWT authentication
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer_auth",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("JWT")
                        .build(),
                ),
            );
        }
    }
}

/// `ZLayer` API `OpenAPI` documentation
#[derive(OpenApi)]
#[openapi(
    info(
        title = "ZLayer API",
        description = "Container orchestration API for ZLayer",
        version = "0.1.0",
        license(name = "MIT OR Apache-2.0"),
        contact(
            name = "ZLayer",
            url = "https://zlayer.dev"
        )
    ),
    paths(
        // Health
        liveness,
        readiness,
        // Auth
        get_token,
        bootstrap,
        login,
        logout,
        me,
        csrf,
        // Users
        list_users,
        create_user,
        get_user,
        update_user,
        delete_user,
        set_password,
        // Deployments
        list_deployments,
        get_deployment,
        create_deployment,
        delete_deployment,
        // Services
        list_services,
        get_service,
        scale_service,
        get_service_logs,
        // Build
        start_build,
        start_build_json,
        get_build_status,
        stream_build,
        get_build_logs,
        list_builds,
        list_runtime_templates,
        // Containers (raw lifecycle)
        create_container,
        list_containers,
        get_container,
        delete_container,
        get_container_logs,
        exec_in_container,
        wait_container,
        get_container_stats,
        // Internal (scheduler-to-agent)
        scale_service_internal,
        get_replicas_internal,
        // Secrets
        create_secret,
        list_secrets,
        get_secret_metadata,
        delete_secret,
        bulk_import_secrets,
        // Environments
        list_environments,
        create_environment,
        get_environment,
        update_environment,
        delete_environment,
        // Projects
        list_projects,
        create_project,
        get_project,
        update_project,
        delete_project,
        list_project_deployments,
        link_project_deployment,
        unlink_project_deployment,
        pull_project,
        // Webhooks
        receive_webhook,
        get_webhook_info,
        rotate_webhook_secret,
        // Credentials
        list_registry_credentials,
        create_registry_credential,
        delete_registry_credential,
        list_git_credentials,
        create_git_credential,
        delete_git_credential,
        // Syncs
        list_syncs,
        create_sync,
        diff_sync,
        apply_sync,
        delete_sync,
        // Variables
        list_variables,
        create_variable,
        get_variable,
        update_variable,
        delete_variable,
        // Tasks
        list_tasks,
        create_task,
        get_task,
        delete_task,
        run_task,
        list_task_runs,
        // Workflows
        list_workflows,
        create_workflow,
        get_workflow,
        delete_workflow,
        run_workflow,
        list_workflow_runs,
        // Notifiers
        list_notifiers,
        create_notifier,
        get_notifier,
        update_notifier,
        delete_notifier,
        test_notifier,
        // Groups
        list_groups,
        create_group,
        get_group,
        update_group,
        delete_group,
        add_member,
        remove_member,
        // Permissions
        list_permissions,
        grant_permission,
        revoke_permission,
        // Audit
        list_audit,
    ),
    components(
        schemas(
            HealthResponse,
            TokenRequest,
            TokenResponse,
            // Auth schemas
            BootstrapRequest,
            LoginRequest,
            LoginResponse,
            UserView,
            CsrfResponse,
            // Users schemas
            CreateUserRequest,
            UpdateUserRequest,
            SetPasswordRequest,
            DeploymentSummary,
            DeploymentDetails,
            CreateDeploymentRequest,
            ServiceSummary,
            ServiceDetails,
            ServiceEndpoint,
            ServiceMetrics,
            ScaleRequest,
            // Build schemas
            BuildRequest,
            BuildRequestWithContext,
            BuildStatus,
            BuildStateEnum,
            TemplateInfo,
            TriggerBuildResponse,
            // Internal schemas
            InternalScaleRequest,
            InternalScaleResponse,
            // Container schemas
            CreateContainerRequest,
            ContainerInfo,
            ContainerResourceLimits,
            VolumeMount,
            ContainerExecRequest,
            ContainerExecResponse,
            ContainerWaitResponse,
            ContainerStatsResponse,
            // Secrets schemas
            CreateSecretRequest,
            SecretMetadataResponse,
            BulkImportResponse,
            // Environments schemas
            crate::storage::StoredEnvironment,
            CreateEnvironmentRequest,
            UpdateEnvironmentRequest,
            // Projects schemas
            crate::storage::StoredProject,
            crate::storage::BuildKind,
            CreateProjectRequest,
            UpdateProjectRequest,
            LinkDeploymentRequest,
            ProjectPullResponse,
            // Webhook schemas
            WebhookResponse,
            WebhookInfoResponse,
            // Credentials schemas
            RegistryCredentialResponse,
            RegistryAuthTypeSchema,
            CreateRegistryCredentialRequest,
            GitCredentialResponse,
            GitCredentialKindSchema,
            CreateGitCredentialRequest,
            // Sync schemas
            crate::storage::StoredSync,
            CreateSyncRequest,
            SyncDiffResponse,
            SyncResourceResponse,
            SyncApplyResponse,
            SyncResourceResult,
            // Variable schemas
            crate::storage::StoredVariable,
            CreateVariableRequest,
            UpdateVariableRequest,
            // Task schemas
            crate::storage::StoredTask,
            crate::storage::TaskKind,
            crate::storage::TaskRun,
            CreateTaskRequest,
            // Workflow schemas
            crate::storage::StoredWorkflow,
            crate::storage::WorkflowStep,
            crate::storage::WorkflowAction,
            crate::storage::WorkflowRun,
            crate::storage::WorkflowRunStatus,
            crate::storage::StepResult,
            CreateWorkflowRequest,
            // Notifier schemas
            crate::storage::StoredNotifier,
            crate::storage::NotifierKind,
            crate::storage::NotifierConfig,
            CreateNotifierRequest,
            UpdateNotifierRequest,
            TestNotifierResponse,
            // Group schemas
            crate::storage::StoredUserGroup,
            CreateGroupRequest,
            UpdateGroupRequest,
            AddMemberRequest,
            GroupMembersResponse,
            // Permission schemas
            crate::storage::StoredPermission,
            crate::storage::SubjectKind,
            crate::storage::PermissionLevel,
            GrantPermissionRequest,
            // Audit schemas
            crate::storage::AuditEntry,
        )
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "Health", description = "Health check endpoints"),
        (name = "Authentication", description = "Authentication endpoints"),
        (name = "Users", description = "User CRUD and password management"),
        (name = "Deployments", description = "Deployment management"),
        (name = "Services", description = "Service management"),
        (name = "Build", description = "Container image building"),
        (name = "Containers", description = "Raw container lifecycle management"),
        (name = "Internal", description = "Internal scheduler-to-agent communication"),
        (name = "Secrets", description = "Secrets management"),
        (name = "Environments", description = "Environment CRUD"),
        (name = "Projects", description = "Project CRUD and deployment linking"),
        (name = "Webhooks", description = "Git push webhook receiver and configuration"),
        (name = "Credentials", description = "Registry and git credential management"),
        (name = "Syncs", description = "GitOps sync management"),
        (name = "Variables", description = "Plaintext variable management"),
        (name = "Tasks", description = "Named runnable script management and execution"),
        (name = "Workflows", description = "Workflow DAG management and execution"),
        (name = "Notifiers", description = "Notification channel management and testing"),
        (name = "Groups", description = "User group CRUD and membership management"),
        (name = "Permissions", description = "Resource-level permission grant/revoke"),
        (name = "Audit", description = "Audit log query"),
    )
)]
pub struct ApiDoc;

#[cfg(test)]
mod tests {
    use super::*;
    use utoipa::OpenApi;

    #[test]
    fn test_openapi_generation() {
        let doc = ApiDoc::openapi();
        assert_eq!(doc.info.title, "ZLayer API");
        assert_eq!(doc.info.version, "0.1.0");
    }

    #[test]
    fn test_openapi_has_paths() {
        let doc = ApiDoc::openapi();
        assert!(!doc.paths.paths.is_empty());
    }

    #[test]
    fn test_openapi_has_security_schemes() {
        let doc = ApiDoc::openapi();
        let components = doc.components.as_ref().expect("should have components");
        assert!(components.security_schemes.contains_key("bearer_auth"));
    }

    #[test]
    fn test_openapi_json_serialization() {
        let doc = ApiDoc::openapi();
        let json = doc.to_json().expect("should serialize to JSON");
        assert!(json.contains("ZLayer API"));
        assert!(json.contains("bearer_auth"));
    }
}
