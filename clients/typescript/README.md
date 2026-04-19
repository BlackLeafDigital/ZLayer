# @zlayer/client@0.1.0

A TypeScript SDK client for the localhost API.

## Usage

First, install the SDK from npm.

```bash
npm install @zlayer/client --save
```

Next, try it out.


```ts
import {
  Configuration,
  AuditApi,
} from '@zlayer/client';
import type { ListAuditRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new AuditApi();

  const body = {
    // string | Filter by user id. (optional)
    user: user_example,
    // string | Filter by resource kind. (optional)
    resourceKind: resourceKind_example,
    // string | Only entries at or after this timestamp (RFC 3339). (optional)
    since: since_example,
    // string | Only entries at or before this timestamp (RFC 3339). (optional)
    until: until_example,
    // number | Maximum number of entries to return (default 100). (optional)
    limit: 56,
  } satisfies ListAuditRequest;

  try {
    const data = await api.listAudit(body);
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```


## Documentation

### API Endpoints

All URIs are relative to *http://localhost*

| Class | Method | HTTP request | Description
| ----- | ------ | ------------ | -------------
*AuditApi* | [**listAudit**](docs/AuditApi.md#listaudit) | **GET** /api/v1/audit | List audit log entries. Admin only.
*AuthenticationApi* | [**bootstrap**](docs/AuthenticationApi.md#bootstrapoperation) | **POST** /auth/bootstrap | Bootstrap the very first admin user. Returns 409 if any user exists.
*AuthenticationApi* | [**csrf**](docs/AuthenticationApi.md#csrf) | **GET** /auth/csrf | Rotate the CSRF double-submit token for the current session.
*AuthenticationApi* | [**getToken**](docs/AuthenticationApi.md#gettoken) | **POST** /auth/token | Get an access token.
*AuthenticationApi* | [**login**](docs/AuthenticationApi.md#loginoperation) | **POST** /auth/login | Sign in an existing user.
*AuthenticationApi* | [**logout**](docs/AuthenticationApi.md#logout) | **POST** /auth/logout | Clear the session + CSRF cookies.
*AuthenticationApi* | [**me**](docs/AuthenticationApi.md#me) | **GET** /auth/me | Return the currently signed-in user.
*BuildApi* | [**getBuildLogs**](docs/BuildApi.md#getbuildlogs) | **GET** /api/v1/build/{id}/logs | GET /api/v1/build/{id}/logs Get build logs.
*BuildApi* | [**getBuildStatus**](docs/BuildApi.md#getbuildstatus) | **GET** /api/v1/build/{id} | GET /api/v1/build/{id} Get build status.
*BuildApi* | [**listBuilds**](docs/BuildApi.md#listbuilds) | **GET** /api/v1/builds | GET /api/v1/builds List all builds.
*BuildApi* | [**listRuntimeTemplates**](docs/BuildApi.md#listruntimetemplates) | **GET** /api/v1/templates | GET /api/v1/templates List available runtime templates
*BuildApi* | [**startBuild**](docs/BuildApi.md#startbuild) | **POST** /api/v1/build | POST /api/v1/build Start a new build from multipart upload (Dockerfile + context tarball)
*BuildApi* | [**startBuildJson**](docs/BuildApi.md#startbuildjson) | **POST** /api/v1/build/json | POST /api/v1/build/json Start a new build from JSON request with a context path on the server.
*BuildApi* | [**streamBuild**](docs/BuildApi.md#streambuild) | **GET** /api/v1/build/{id}/stream | GET /api/v1/build/{id}/stream Stream build progress via Server-Sent Events.
*ClusterApi* | [**clusterForceLeader**](docs/ClusterApi.md#clusterforceleader) | **POST** /api/v1/cluster/force-leader | Force this node to become the cluster leader (disaster recovery).
*ClusterApi* | [**clusterHeartbeat**](docs/ClusterApi.md#clusterheartbeat) | **POST** /api/v1/cluster/heartbeat | Handle node heartbeat.
*ClusterApi* | [**clusterJoin**](docs/ClusterApi.md#clusterjoinoperation) | **POST** /api/v1/cluster/join | Handle a cluster join request.
*ClusterApi* | [**clusterListNodes**](docs/ClusterApi.md#clusterlistnodes) | **GET** /api/v1/cluster/nodes | List all nodes visible in the Raft cluster state.
*ContainersApi* | [**createContainer**](docs/ContainersApi.md#createcontaineroperation) | **POST** /api/v1/containers | Create and start a container.
*ContainersApi* | [**deleteContainer**](docs/ContainersApi.md#deletecontainer) | **DELETE** /api/v1/containers/{id} | Stop and remove a container.
*ContainersApi* | [**execInContainer**](docs/ContainersApi.md#execincontainer) | **POST** /api/v1/containers/{id}/exec | Execute a command in a running container.
*ContainersApi* | [**getContainer**](docs/ContainersApi.md#getcontainer) | **GET** /api/v1/containers/{id} | Get details for a specific container.
*ContainersApi* | [**getContainerLogs**](docs/ContainersApi.md#getcontainerlogs) | **GET** /api/v1/containers/{id}/logs | Get container logs.
*ContainersApi* | [**getContainerStats**](docs/ContainersApi.md#getcontainerstats) | **GET** /api/v1/containers/{id}/stats | Get container resource statistics.
*ContainersApi* | [**killContainer**](docs/ContainersApi.md#killcontaineroperation) | **POST** /api/v1/containers/{id}/kill | Send a signal to a running container.
*ContainersApi* | [**listContainers**](docs/ContainersApi.md#listcontainers) | **GET** /api/v1/containers | List standalone containers.
*ContainersApi* | [**restartContainer**](docs/ContainersApi.md#restartcontaineroperation) | **POST** /api/v1/containers/{id}/restart | Restart a container: stop then start.
*ContainersApi* | [**startContainer**](docs/ContainersApi.md#startcontainer) | **POST** /api/v1/containers/{id}/start | Start a previously-created container.
*ContainersApi* | [**stopContainer**](docs/ContainersApi.md#stopcontaineroperation) | **POST** /api/v1/containers/{id}/stop | Stop a running container.
*ContainersApi* | [**waitContainer**](docs/ContainersApi.md#waitcontainer) | **GET** /api/v1/containers/{id}/wait | Wait for a container to exit and return its exit code.
*CredentialsApi* | [**createGitCredential**](docs/CredentialsApi.md#creategitcredentialoperation) | **POST** /api/v1/credentials/git | Create a new git credential. Admin only.
*CredentialsApi* | [**createRegistryCredential**](docs/CredentialsApi.md#createregistrycredentialoperation) | **POST** /api/v1/credentials/registry | Create a new registry credential. Admin only.
*CredentialsApi* | [**deleteGitCredential**](docs/CredentialsApi.md#deletegitcredential) | **DELETE** /api/v1/credentials/git/{id} | Delete a git credential. Admin only.
*CredentialsApi* | [**deleteRegistryCredential**](docs/CredentialsApi.md#deleteregistrycredential) | **DELETE** /api/v1/credentials/registry/{id} | Delete a registry credential. Admin only.
*CredentialsApi* | [**listGitCredentials**](docs/CredentialsApi.md#listgitcredentials) | **GET** /api/v1/credentials/git | List all git credentials (metadata only, no secret values).
*CredentialsApi* | [**listRegistryCredentials**](docs/CredentialsApi.md#listregistrycredentials) | **GET** /api/v1/credentials/registry | List all registry credentials (metadata only, no passwords).
*CronApi* | [**disableCronJob**](docs/CronApi.md#disablecronjob) | **PUT** /api/v1/cron/{name}/disable | PUT /api/v1/cron/{name}/disable - Disable a cron job
*CronApi* | [**enableCronJob**](docs/CronApi.md#enablecronjob) | **PUT** /api/v1/cron/{name}/enable | PUT /api/v1/cron/{name}/enable - Enable a cron job
*CronApi* | [**getCronJob**](docs/CronApi.md#getcronjob) | **GET** /api/v1/cron/{name} | GET /api/v1/cron/{name} - Get cron job details
*CronApi* | [**listCronJobs**](docs/CronApi.md#listcronjobs) | **GET** /api/v1/cron | GET /api/v1/cron - List all cron jobs
*CronApi* | [**triggerCronJob**](docs/CronApi.md#triggercronjob) | **POST** /api/v1/cron/{name}/trigger | POST /api/v1/cron/{name}/trigger - Manually trigger a cron job
*DeploymentsApi* | [**createDeployment**](docs/DeploymentsApi.md#createdeploymentoperation) | **POST** /api/v1/deployments | Create a new deployment.
*DeploymentsApi* | [**deleteDeployment**](docs/DeploymentsApi.md#deletedeployment) | **DELETE** /api/v1/deployments/{name} | Delete a deployment.
*DeploymentsApi* | [**getDeployment**](docs/DeploymentsApi.md#getdeployment) | **GET** /api/v1/deployments/{name} | Get deployment details (with live per-service health when available).
*DeploymentsApi* | [**listDeployments**](docs/DeploymentsApi.md#listdeployments) | **GET** /api/v1/deployments | List all deployments.
*EnvironmentsApi* | [**createEnvironment**](docs/EnvironmentsApi.md#createenvironmentoperation) | **POST** /api/v1/environments | Create a new environment. Admin only.
*EnvironmentsApi* | [**deleteEnvironment**](docs/EnvironmentsApi.md#deleteenvironment) | **DELETE** /api/v1/environments/{id} | Delete an environment. Admin only.
*EnvironmentsApi* | [**getEnvironment**](docs/EnvironmentsApi.md#getenvironment) | **GET** /api/v1/environments/{id} | Fetch a single environment by id.
*EnvironmentsApi* | [**listEnvironments**](docs/EnvironmentsApi.md#listenvironments) | **GET** /api/v1/environments | List environments.
*EnvironmentsApi* | [**updateEnvironment**](docs/EnvironmentsApi.md#updateenvironmentoperation) | **PATCH** /api/v1/environments/{id} | Rename / re-describe an environment. Admin only.
*GroupsApi* | [**addMember**](docs/GroupsApi.md#addmemberoperation) | **POST** /api/v1/groups/{id}/members | Add a member to a group. Admin only.
*GroupsApi* | [**createGroup**](docs/GroupsApi.md#creategroupoperation) | **POST** /api/v1/groups | Create a new group. Admin only.
*GroupsApi* | [**deleteGroup**](docs/GroupsApi.md#deletegroup) | **DELETE** /api/v1/groups/{id} | Delete a group. Admin only.
*GroupsApi* | [**getGroup**](docs/GroupsApi.md#getgroup) | **GET** /api/v1/groups/{id} | Fetch a single group by id.
*GroupsApi* | [**listGroups**](docs/GroupsApi.md#listgroups) | **GET** /api/v1/groups | List all groups.
*GroupsApi* | [**removeMember**](docs/GroupsApi.md#removemember) | **DELETE** /api/v1/groups/{id}/members/{user_id} | Remove a member from a group. Admin only.
*GroupsApi* | [**updateGroup**](docs/GroupsApi.md#updategroupoperation) | **PATCH** /api/v1/groups/{id} | Update a group\&#39;s name and/or description. Admin only.
*HealthApi* | [**liveness**](docs/HealthApi.md#liveness) | **GET** /health/live | Liveness probe - basic health check
*HealthApi* | [**readiness**](docs/HealthApi.md#readiness) | **GET** /health/ready | Readiness probe - full health check
*ImagesApi* | [**listImagesHandler**](docs/ImagesApi.md#listimageshandler) | **GET** /api/v1/images | List all cached images known to the runtime.
*ImagesApi* | [**pruneImagesHandler**](docs/ImagesApi.md#pruneimageshandler) | **POST** /api/v1/system/prune | Prune dangling / unused images from the runtime\&#39;s cache.
*ImagesApi* | [**pullImageHandler**](docs/ImagesApi.md#pullimagehandler) | **POST** /api/v1/images/pull | Pull an OCI image into the runtime\&#39;s local cache.
*ImagesApi* | [**removeImageHandler**](docs/ImagesApi.md#removeimagehandler) | **DELETE** /api/v1/images/{image} | Remove an image from the runtime\&#39;s cache.
*ImagesApi* | [**tagImageHandler**](docs/ImagesApi.md#tagimagehandler) | **POST** /api/v1/images/tag | Create a new tag pointing at an existing image.
*InternalApi* | [**getReplicasInternal**](docs/InternalApi.md#getreplicasinternal) | **GET** /api/v1/internal/replicas/{service} | Get the current replica count for a service.
*InternalApi* | [**scaleServiceInternal**](docs/InternalApi.md#scaleserviceinternal) | **POST** /api/v1/internal/scale | Scale a service via internal scheduler request.
*JobsApi* | [**cancelExecution**](docs/JobsApi.md#cancelexecution) | **POST** /api/v1/jobs/{execution_id}/cancel | POST /&#x60;api/v1/jobs/{execution_id}/cancel&#x60; - Cancel a running execution
*JobsApi* | [**getExecutionStatus**](docs/JobsApi.md#getexecutionstatus) | **GET** /api/v1/jobs/{execution_id}/status | GET /&#x60;api/v1/jobs/{execution_id}/status&#x60; - Get execution status
*JobsApi* | [**listJobExecutions**](docs/JobsApi.md#listjobexecutions) | **GET** /api/v1/jobs/{name}/executions | GET /api/v1/jobs/{name}/executions - List executions for a job
*JobsApi* | [**triggerJob**](docs/JobsApi.md#triggerjob) | **POST** /api/v1/jobs/{name}/trigger | POST /api/v1/jobs/{name}/trigger - Trigger a job execution
*NetworksApi* | [**createNetwork**](docs/NetworksApi.md#createnetwork) | **POST** /api/v1/networks | Create a new network.
*NetworksApi* | [**deleteNetwork**](docs/NetworksApi.md#deletenetwork) | **DELETE** /api/v1/networks/{name} | Delete a network.
*NetworksApi* | [**getNetwork**](docs/NetworksApi.md#getnetwork) | **GET** /api/v1/networks/{name} | Get a specific network by name.
*NetworksApi* | [**listNetworks**](docs/NetworksApi.md#listnetworks) | **GET** /api/v1/networks | List all networks.
*NetworksApi* | [**updateNetwork**](docs/NetworksApi.md#updatenetwork) | **PUT** /api/v1/networks/{name} | Update an existing network.
*NodesApi* | [**generateJoinToken**](docs/NodesApi.md#generatejointoken) | **POST** /api/v1/nodes/join-token | Generate a join token for new nodes to join the cluster.
*NodesApi* | [**getNode**](docs/NodesApi.md#getnode) | **GET** /api/v1/nodes/{id} | Get detailed information about a specific node.
*NodesApi* | [**listNodes**](docs/NodesApi.md#listnodes) | **GET** /api/v1/nodes | List all nodes in the cluster.
*NodesApi* | [**updateNodeLabels**](docs/NodesApi.md#updatenodelabels) | **POST** /api/v1/nodes/{id}/labels | Update labels on a node.
*NotifiersApi* | [**createNotifier**](docs/NotifiersApi.md#createnotifieroperation) | **POST** /api/v1/notifiers | Create a new notifier. Admin only.
*NotifiersApi* | [**deleteNotifier**](docs/NotifiersApi.md#deletenotifier) | **DELETE** /api/v1/notifiers/{id} | Delete a notifier. Admin only.
*NotifiersApi* | [**getNotifier**](docs/NotifiersApi.md#getnotifier) | **GET** /api/v1/notifiers/{id} | Fetch a single notifier by id.
*NotifiersApi* | [**listNotifiers**](docs/NotifiersApi.md#listnotifiers) | **GET** /api/v1/notifiers | List notifiers.
*NotifiersApi* | [**testNotifier**](docs/NotifiersApi.md#testnotifier) | **POST** /api/v1/notifiers/{id}/test | Send a test notification through a notifier. Admin only.
*NotifiersApi* | [**updateNotifier**](docs/NotifiersApi.md#updatenotifieroperation) | **PATCH** /api/v1/notifiers/{id} | Update a notifier. Admin only.
*OverlayApi* | [**getDnsStatus**](docs/OverlayApi.md#getdnsstatus) | **GET** /api/v1/overlay/dns | Get DNS service status.
*OverlayApi* | [**getIpAllocation**](docs/OverlayApi.md#getipallocation) | **GET** /api/v1/overlay/ip-alloc | Get IP allocation status.
*OverlayApi* | [**getOverlayPeers**](docs/OverlayApi.md#getoverlaypeers) | **GET** /api/v1/overlay/peers | Get list of overlay peers.
*OverlayApi* | [**getOverlayStatus**](docs/OverlayApi.md#getoverlaystatus) | **GET** /api/v1/overlay/status | Get overlay network status.
*PermissionsApi* | [**grantPermission**](docs/PermissionsApi.md#grantpermissionoperation) | **POST** /api/v1/permissions | Grant a permission. Admin only.
*PermissionsApi* | [**listPermissions**](docs/PermissionsApi.md#listpermissions) | **GET** /api/v1/permissions | List permissions for a subject (user or group).
*PermissionsApi* | [**revokePermission**](docs/PermissionsApi.md#revokepermission) | **DELETE** /api/v1/permissions/{id} | Revoke a permission by id. Admin only.
*ProjectsApi* | [**createProject**](docs/ProjectsApi.md#createprojectoperation) | **POST** /api/v1/projects | Create a new project. Admin only.
*ProjectsApi* | [**deleteProject**](docs/ProjectsApi.md#deleteproject) | **DELETE** /api/v1/projects/{id} | Delete a project. Admin only. Cascade-removes deployment links.
*ProjectsApi* | [**getProject**](docs/ProjectsApi.md#getproject) | **GET** /api/v1/projects/{id} | Fetch a single project by id.
*ProjectsApi* | [**linkProjectDeployment**](docs/ProjectsApi.md#linkprojectdeployment) | **POST** /api/v1/projects/{id}/deployments | Link a deployment to a project.
*ProjectsApi* | [**listProjectDeployments**](docs/ProjectsApi.md#listprojectdeployments) | **GET** /api/v1/projects/{id}/deployments | List deployment names linked to a project.
*ProjectsApi* | [**listProjects**](docs/ProjectsApi.md#listprojects) | **GET** /api/v1/projects | List all projects.
*ProjectsApi* | [**pullProject**](docs/ProjectsApi.md#pullproject) | **POST** /api/v1/projects/{id}/pull | Clone the project\&#39;s git repository (or fast-forward pull if the working copy already exists) into &#x60;{clone_root}/{project_id}&#x60; and return the resulting HEAD SHA.
*ProjectsApi* | [**unlinkProjectDeployment**](docs/ProjectsApi.md#unlinkprojectdeployment) | **DELETE** /api/v1/projects/{id}/deployments/{name} | Unlink a deployment from a project.
*ProjectsApi* | [**updateProject**](docs/ProjectsApi.md#updateprojectoperation) | **PATCH** /api/v1/projects/{id} | Update a project. Admin only.
*ProxyApi* | [**listBackends**](docs/ProxyApi.md#listbackends) | **GET** /api/v1/proxy/backends | List all load-balancer backend groups.
*ProxyApi* | [**listRoutes**](docs/ProxyApi.md#listroutes) | **GET** /api/v1/proxy/routes | List all registered proxy routes.
*ProxyApi* | [**listStreams**](docs/ProxyApi.md#liststreams) | **GET** /api/v1/proxy/streams | List L4 stream proxies.
*ProxyApi* | [**listTls**](docs/ProxyApi.md#listtls) | **GET** /api/v1/proxy/tls | List loaded TLS certificates.
*SecretsApi* | [**bulkImportSecrets**](docs/SecretsApi.md#bulkimportsecrets) | **POST** /api/v1/secrets/bulk-import | Bulk-import secrets from a dotenv-style payload (&#x60;KEY&#x3D;value\\n…&#x60;).
*SecretsApi* | [**createSecret**](docs/SecretsApi.md#createsecretoperation) | **POST** /api/v1/secrets | Create or update a secret.
*SecretsApi* | [**deleteSecret**](docs/SecretsApi.md#deletesecret) | **DELETE** /api/v1/secrets/{name} | Delete a secret.
*SecretsApi* | [**getSecretMetadata**](docs/SecretsApi.md#getsecretmetadata) | **GET** /api/v1/secrets/{name} | Get metadata for a specific secret. With &#x60;?reveal&#x3D;true&#x60; (admin only), the response also includes the plaintext &#x60;value&#x60;.
*SecretsApi* | [**listSecrets**](docs/SecretsApi.md#listsecrets) | **GET** /api/v1/secrets | List secrets in a scope.
*ServicesApi* | [**getService**](docs/ServicesApi.md#getservice) | **GET** /api/v1/deployments/{deployment}/services/{service} | Get service details.
*ServicesApi* | [**getServiceLogs**](docs/ServicesApi.md#getservicelogs) | **GET** /api/v1/deployments/{deployment}/services/{service}/logs | Get service logs.
*ServicesApi* | [**listServices**](docs/ServicesApi.md#listservices) | **GET** /api/v1/deployments/{deployment}/services | List services in a deployment.
*ServicesApi* | [**scaleService**](docs/ServicesApi.md#scaleservice) | **POST** /api/v1/deployments/{deployment}/services/{service}/scale | Scale a service.
*StorageApi* | [**getStorageStatus**](docs/StorageApi.md#getstoragestatus) | **GET** /api/v1/storage/status | Get storage replication status.
*SyncsApi* | [**applySync**](docs/SyncsApi.md#applysync) | **POST** /api/v1/syncs/{id}/apply | Apply a sync — real reconcile against the API.
*SyncsApi* | [**createSync**](docs/SyncsApi.md#createsyncoperation) | **POST** /api/v1/syncs | Create a new sync.
*SyncsApi* | [**deleteSync**](docs/SyncsApi.md#deletesync) | **DELETE** /api/v1/syncs/{id} | Delete a sync.
*SyncsApi* | [**diffSync**](docs/SyncsApi.md#diffsync) | **GET** /api/v1/syncs/{id}/diff | Compute a diff for a sync (scan git path vs. remote resources).
*SyncsApi* | [**listSyncs**](docs/SyncsApi.md#listsyncs) | **GET** /api/v1/syncs | List all syncs.
*TasksApi* | [**createTask**](docs/TasksApi.md#createtaskoperation) | **POST** /api/v1/tasks | Create a new task. Admin only.
*TasksApi* | [**deleteTask**](docs/TasksApi.md#deletetask) | **DELETE** /api/v1/tasks/{id} | Delete a task. Admin only.
*TasksApi* | [**getTask**](docs/TasksApi.md#gettask) | **GET** /api/v1/tasks/{id} | Fetch a single task by id.
*TasksApi* | [**listTaskRuns**](docs/TasksApi.md#listtaskruns) | **GET** /api/v1/tasks/{id}/runs | List past runs for a task, most recent first.
*TasksApi* | [**listTasks**](docs/TasksApi.md#listtasks) | **GET** /api/v1/tasks | List tasks.
*TasksApi* | [**runTask**](docs/TasksApi.md#runtask) | **POST** /api/v1/tasks/{id}/run | Execute a task synchronously. Admin only.
*TunnelsApi* | [**createNodeTunnel**](docs/TunnelsApi.md#createnodetunneloperation) | **POST** /api/v1/tunnels/node | Create a node-to-node tunnel.
*TunnelsApi* | [**createTunnel**](docs/TunnelsApi.md#createtunneloperation) | **POST** /api/v1/tunnels | Create a new tunnel token.
*TunnelsApi* | [**getTunnelStatus**](docs/TunnelsApi.md#gettunnelstatus) | **GET** /api/v1/tunnels/{id}/status | Get tunnel status.
*TunnelsApi* | [**listTunnels**](docs/TunnelsApi.md#listtunnels) | **GET** /api/v1/tunnels | List all tunnels.
*TunnelsApi* | [**removeNodeTunnel**](docs/TunnelsApi.md#removenodetunnel) | **DELETE** /api/v1/tunnels/node/{name} | Remove a node-to-node tunnel.
*TunnelsApi* | [**revokeTunnel**](docs/TunnelsApi.md#revoketunnel) | **DELETE** /api/v1/tunnels/{id} | Revoke (delete) a tunnel.
*UsersApi* | [**createUser**](docs/UsersApi.md#createuseroperation) | **POST** /api/v1/users | Create a new user. Admin only.
*UsersApi* | [**deleteUser**](docs/UsersApi.md#deleteuser) | **DELETE** /api/v1/users/{id} | Delete a user. Admin only. Callers cannot delete their own account.
*UsersApi* | [**getUser**](docs/UsersApi.md#getuser) | **GET** /api/v1/users/{id} | Fetch a single user. Admins can read any record; regular users can read only their own.
*UsersApi* | [**listUsers**](docs/UsersApi.md#listusers) | **GET** /api/v1/users | List all users. Admin only.
*UsersApi* | [**setPassword**](docs/UsersApi.md#setpasswordoperation) | **POST** /api/v1/users/{id}/password | Set a user\&#39;s password. Admins may change any user\&#39;s password; regular users may only change their own, and must supply &#x60;current_password&#x60;.
*UsersApi* | [**updateUser**](docs/UsersApi.md#updateuseroperation) | **PATCH** /api/v1/users/{id} | Update a user\&#39;s mutable fields. Admin only.
*VariablesApi* | [**createVariable**](docs/VariablesApi.md#createvariableoperation) | **POST** /api/v1/variables | Create a new variable. Admin only.
*VariablesApi* | [**deleteVariable**](docs/VariablesApi.md#deletevariable) | **DELETE** /api/v1/variables/{id} | Delete a variable. Admin only.
*VariablesApi* | [**getVariable**](docs/VariablesApi.md#getvariable) | **GET** /api/v1/variables/{id} | Fetch a single variable by id.
*VariablesApi* | [**listVariables**](docs/VariablesApi.md#listvariables) | **GET** /api/v1/variables | List variables.
*VariablesApi* | [**updateVariable**](docs/VariablesApi.md#updatevariableoperation) | **PATCH** /api/v1/variables/{id} | Update a variable\&#39;s name and/or value. Admin only.
*VolumesApi* | [**deleteVolume**](docs/VolumesApi.md#deletevolume) | **DELETE** /api/v1/volumes/{name} | Delete a volume by name.
*VolumesApi* | [**listVolumes**](docs/VolumesApi.md#listvolumes) | **GET** /api/v1/volumes | List all volumes on disk.
*WebhooksApi* | [**getWebhookInfo**](docs/WebhooksApi.md#getwebhookinfo) | **GET** /api/v1/projects/{id}/webhook | Get webhook configuration for a project.
*WebhooksApi* | [**receiveWebhook**](docs/WebhooksApi.md#receivewebhook) | **POST** /webhooks/{provider}/{project_id} | Receive a webhook push event and trigger a project pull.
*WebhooksApi* | [**rotateWebhookSecret**](docs/WebhooksApi.md#rotatewebhooksecret) | **POST** /api/v1/projects/{id}/webhook/rotate | Rotate (regenerate) the webhook secret for a project.
*WorkflowsApi* | [**createWorkflow**](docs/WorkflowsApi.md#createworkflowoperation) | **POST** /api/v1/workflows | Create a new workflow. Admin only.
*WorkflowsApi* | [**deleteWorkflow**](docs/WorkflowsApi.md#deleteworkflow) | **DELETE** /api/v1/workflows/{id} | Delete a workflow. Admin only.
*WorkflowsApi* | [**getWorkflow**](docs/WorkflowsApi.md#getworkflow) | **GET** /api/v1/workflows/{id} | Fetch a single workflow by id.
*WorkflowsApi* | [**listWorkflowRuns**](docs/WorkflowsApi.md#listworkflowruns) | **GET** /api/v1/workflows/{id}/runs | List past runs for a workflow, most recent first.
*WorkflowsApi* | [**listWorkflows**](docs/WorkflowsApi.md#listworkflows) | **GET** /api/v1/workflows | List workflows.
*WorkflowsApi* | [**runWorkflow**](docs/WorkflowsApi.md#runworkflow) | **POST** /api/v1/workflows/{id}/run | Execute a workflow synchronously. Admin only.


### Models

- [AddMemberRequest](docs/AddMemberRequest.md)
- [AuditEntry](docs/AuditEntry.md)
- [BackendGroupInfo](docs/BackendGroupInfo.md)
- [BackendInfo](docs/BackendInfo.md)
- [BackendsResponse](docs/BackendsResponse.md)
- [BootstrapRequest](docs/BootstrapRequest.md)
- [BuildKind](docs/BuildKind.md)
- [BuildRequest](docs/BuildRequest.md)
- [BuildRequestWithContext](docs/BuildRequestWithContext.md)
- [BuildStateEnum](docs/BuildStateEnum.md)
- [BuildStatus](docs/BuildStatus.md)
- [BulkImportResponse](docs/BulkImportResponse.md)
- [CertInfo](docs/CertInfo.md)
- [ClusterJoinRequest](docs/ClusterJoinRequest.md)
- [ClusterJoinResponse](docs/ClusterJoinResponse.md)
- [ClusterNodeSummary](docs/ClusterNodeSummary.md)
- [ClusterPeer](docs/ClusterPeer.md)
- [ContainerExecRequest](docs/ContainerExecRequest.md)
- [ContainerExecResponse](docs/ContainerExecResponse.md)
- [ContainerInfo](docs/ContainerInfo.md)
- [ContainerResourceLimits](docs/ContainerResourceLimits.md)
- [ContainerStatsResponse](docs/ContainerStatsResponse.md)
- [ContainerWaitResponse](docs/ContainerWaitResponse.md)
- [CreateContainerRequest](docs/CreateContainerRequest.md)
- [CreateDeploymentRequest](docs/CreateDeploymentRequest.md)
- [CreateEnvironmentRequest](docs/CreateEnvironmentRequest.md)
- [CreateGitCredentialRequest](docs/CreateGitCredentialRequest.md)
- [CreateGroupRequest](docs/CreateGroupRequest.md)
- [CreateNodeTunnelRequest](docs/CreateNodeTunnelRequest.md)
- [CreateNodeTunnelResponse](docs/CreateNodeTunnelResponse.md)
- [CreateNotifierRequest](docs/CreateNotifierRequest.md)
- [CreateProjectRequest](docs/CreateProjectRequest.md)
- [CreateRegistryCredentialRequest](docs/CreateRegistryCredentialRequest.md)
- [CreateSecretRequest](docs/CreateSecretRequest.md)
- [CreateSyncRequest](docs/CreateSyncRequest.md)
- [CreateTaskRequest](docs/CreateTaskRequest.md)
- [CreateTunnelRequest](docs/CreateTunnelRequest.md)
- [CreateTunnelResponse](docs/CreateTunnelResponse.md)
- [CreateUserRequest](docs/CreateUserRequest.md)
- [CreateVariableRequest](docs/CreateVariableRequest.md)
- [CreateWorkflowRequest](docs/CreateWorkflowRequest.md)
- [CronJobResponse](docs/CronJobResponse.md)
- [CronStatusResponse](docs/CronStatusResponse.md)
- [CsrfResponse](docs/CsrfResponse.md)
- [DeploymentDetails](docs/DeploymentDetails.md)
- [DeploymentSummary](docs/DeploymentSummary.md)
- [DnsStatusResponse](docs/DnsStatusResponse.md)
- [ForceLeaderRequest](docs/ForceLeaderRequest.md)
- [ForceLeaderResponse](docs/ForceLeaderResponse.md)
- [GitCredentialKindSchema](docs/GitCredentialKindSchema.md)
- [GitCredentialResponse](docs/GitCredentialResponse.md)
- [GpuInfoSummary](docs/GpuInfoSummary.md)
- [GpuUtilizationReport](docs/GpuUtilizationReport.md)
- [GrantPermissionRequest](docs/GrantPermissionRequest.md)
- [GroupMembersResponse](docs/GroupMembersResponse.md)
- [HealthResponse](docs/HealthResponse.md)
- [HeartbeatRequest](docs/HeartbeatRequest.md)
- [ImageInfoDto](docs/ImageInfoDto.md)
- [InternalScaleRequest](docs/InternalScaleRequest.md)
- [InternalScaleResponse](docs/InternalScaleResponse.md)
- [IpAllocationResponse](docs/IpAllocationResponse.md)
- [JobExecutionResponse](docs/JobExecutionResponse.md)
- [JoinTokenResponse](docs/JoinTokenResponse.md)
- [KillContainerRequest](docs/KillContainerRequest.md)
- [LinkDeploymentRequest](docs/LinkDeploymentRequest.md)
- [LoginRequest](docs/LoginRequest.md)
- [LoginResponse](docs/LoginResponse.md)
- [NetworkSummary](docs/NetworkSummary.md)
- [NodeDetails](docs/NodeDetails.md)
- [NodeResourceInfo](docs/NodeResourceInfo.md)
- [NodeSummary](docs/NodeSummary.md)
- [NotifierConfig](docs/NotifierConfig.md)
- [NotifierConfigOneOf](docs/NotifierConfigOneOf.md)
- [NotifierConfigOneOf1](docs/NotifierConfigOneOf1.md)
- [NotifierConfigOneOf2](docs/NotifierConfigOneOf2.md)
- [NotifierConfigOneOf3](docs/NotifierConfigOneOf3.md)
- [NotifierKind](docs/NotifierKind.md)
- [OverlayStatusResponse](docs/OverlayStatusResponse.md)
- [PeerInfo](docs/PeerInfo.md)
- [PeerListResponse](docs/PeerListResponse.md)
- [PermissionLevel](docs/PermissionLevel.md)
- [ProjectPullResponse](docs/ProjectPullResponse.md)
- [PruneResultDto](docs/PruneResultDto.md)
- [PullImageRequest](docs/PullImageRequest.md)
- [PullImageResponse](docs/PullImageResponse.md)
- [RegisteredServiceInfo](docs/RegisteredServiceInfo.md)
- [RegistryAuthTypeSchema](docs/RegistryAuthTypeSchema.md)
- [RegistryCredentialResponse](docs/RegistryCredentialResponse.md)
- [ReplicationInfo](docs/ReplicationInfo.md)
- [RestartContainerRequest](docs/RestartContainerRequest.md)
- [RouteInfo](docs/RouteInfo.md)
- [RoutesResponse](docs/RoutesResponse.md)
- [ScaleRequest](docs/ScaleRequest.md)
- [SecretMetadataResponse](docs/SecretMetadataResponse.md)
- [ServiceDetails](docs/ServiceDetails.md)
- [ServiceEndpoint](docs/ServiceEndpoint.md)
- [ServiceHealthInfo](docs/ServiceHealthInfo.md)
- [ServiceMetrics](docs/ServiceMetrics.md)
- [ServiceSummary](docs/ServiceSummary.md)
- [SetPasswordRequest](docs/SetPasswordRequest.md)
- [StepResult](docs/StepResult.md)
- [StopContainerRequest](docs/StopContainerRequest.md)
- [StorageStatusResponse](docs/StorageStatusResponse.md)
- [StoredEnvironment](docs/StoredEnvironment.md)
- [StoredNotifier](docs/StoredNotifier.md)
- [StoredPermission](docs/StoredPermission.md)
- [StoredProject](docs/StoredProject.md)
- [StoredSync](docs/StoredSync.md)
- [StoredTask](docs/StoredTask.md)
- [StoredUserGroup](docs/StoredUserGroup.md)
- [StoredVariable](docs/StoredVariable.md)
- [StoredWorkflow](docs/StoredWorkflow.md)
- [StreamBackendInfo](docs/StreamBackendInfo.md)
- [StreamInfo](docs/StreamInfo.md)
- [StreamsResponse](docs/StreamsResponse.md)
- [SubjectKind](docs/SubjectKind.md)
- [SuccessResponse](docs/SuccessResponse.md)
- [SyncApplyResponse](docs/SyncApplyResponse.md)
- [SyncDiffResponse](docs/SyncDiffResponse.md)
- [SyncResourceResponse](docs/SyncResourceResponse.md)
- [SyncResourceResult](docs/SyncResourceResult.md)
- [TagImageRequest](docs/TagImageRequest.md)
- [TaskKind](docs/TaskKind.md)
- [TaskRun](docs/TaskRun.md)
- [TemplateInfo](docs/TemplateInfo.md)
- [TestNotifierResponse](docs/TestNotifierResponse.md)
- [TlsResponse](docs/TlsResponse.md)
- [TokenRequest](docs/TokenRequest.md)
- [TokenResponse](docs/TokenResponse.md)
- [TriggerBuildResponse](docs/TriggerBuildResponse.md)
- [TriggerCronResponse](docs/TriggerCronResponse.md)
- [TriggerJobResponse](docs/TriggerJobResponse.md)
- [TunnelStatus](docs/TunnelStatus.md)
- [TunnelSummary](docs/TunnelSummary.md)
- [UpdateEnvironmentRequest](docs/UpdateEnvironmentRequest.md)
- [UpdateGroupRequest](docs/UpdateGroupRequest.md)
- [UpdateLabelsRequest](docs/UpdateLabelsRequest.md)
- [UpdateLabelsResponse](docs/UpdateLabelsResponse.md)
- [UpdateNotifierRequest](docs/UpdateNotifierRequest.md)
- [UpdateProjectRequest](docs/UpdateProjectRequest.md)
- [UpdateUserRequest](docs/UpdateUserRequest.md)
- [UpdateVariableRequest](docs/UpdateVariableRequest.md)
- [UserRole](docs/UserRole.md)
- [UserView](docs/UserView.md)
- [VolumeMount](docs/VolumeMount.md)
- [VolumeSummary](docs/VolumeSummary.md)
- [WebhookInfoResponse](docs/WebhookInfoResponse.md)
- [WebhookResponse](docs/WebhookResponse.md)
- [WorkflowAction](docs/WorkflowAction.md)
- [WorkflowActionOneOf](docs/WorkflowActionOneOf.md)
- [WorkflowActionOneOf1](docs/WorkflowActionOneOf1.md)
- [WorkflowActionOneOf2](docs/WorkflowActionOneOf2.md)
- [WorkflowActionOneOf3](docs/WorkflowActionOneOf3.md)
- [WorkflowRun](docs/WorkflowRun.md)
- [WorkflowRunStatus](docs/WorkflowRunStatus.md)
- [WorkflowStep](docs/WorkflowStep.md)

### Authorization


Authentication schemes defined for the API:
<a id="bearer_auth"></a>
#### bearer_auth


- **Type**: HTTP Bearer Token authentication (JWT)

## About

This TypeScript SDK client supports the [Fetch API](https://fetch.spec.whatwg.org/)
and is automatically generated by the
[OpenAPI Generator](https://openapi-generator.tech) project:

- API version: `0.1.0`
- Package version: `0.1.0`
- Generator version: `7.21.0`
- Build package: `org.openapitools.codegen.languages.TypeScriptFetchClientCodegen`

The generated npm module supports the following:

- Environments
  * Node.js
  * Webpack
  * Browserify
- Language levels
  * ES5 - you must have a Promises/A+ library installed
  * ES6
- Module systems
  * CommonJS
  * ES6 module system

For more information, please visit [https://zlayer.dev](https://zlayer.dev)

## Development

### Building

To build the TypeScript source code, you need to have Node.js and npm installed.
After cloning the repository, navigate to the project directory and run:

```bash
npm install
npm run build
```

### Publishing

Once you've built the package, you can publish it to npm:

```bash
npm publish
```

## License

[MIT OR Apache-2.0]()
