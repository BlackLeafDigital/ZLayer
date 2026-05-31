# CreateContainerRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**BlkioWeight** | Pointer to **NullableInt32** | Block IO weight, 10-1000 (Docker &#x60;--blkio-weight&#x60;). | [optional] 
**CapAdd** | Pointer to **[]string** | Linux capabilities to add (Docker &#x60;--cap-add&#x60;). Maps to &#x60;ServiceSpec::capabilities&#x60;. | [optional] 
**CapDrop** | Pointer to **[]string** | Linux capabilities to drop (Docker &#x60;--cap-drop&#x60;). | [optional] 
**Command** | Pointer to **[]string** | Command to run (overrides image entrypoint) | [optional] 
**CpuShares** | Pointer to **NullableInt32** | Relative CPU shares (Docker &#x60;--cpu-shares&#x60;). Default weight is 1024. | [optional] 
**Cpuset** | Pointer to **NullableString** | CPUs that the container is allowed to execute on (Docker &#x60;--cpuset-cpus&#x60;). | [optional] 
**Devices** | Pointer to [**[]DeviceSpec**](DeviceSpec.md) | Host devices to expose to the container (Docker &#x60;--device&#x60;). | [optional] 
**Dns** | Pointer to **[]string** | Additional DNS servers (maps to Docker&#39;s &#x60;--dns&#x60;). Each entry must be a plausible IPv4 or IPv6 address. | [optional] 
**Env** | Pointer to **map[string]string** | Environment variables | [optional] 
**ExtraGroups** | Pointer to **[]string** | Additional groups to add to the container process (Docker &#x60;--group-add&#x60;). | [optional] 
**ExtraHosts** | Pointer to **[]string** | Extra &#x60;hostname:ip&#x60; entries appended to &#x60;/etc/hosts&#x60; (maps to Docker&#39;s &#x60;--add-host&#x60;). The special literal &#x60;host-gateway&#x60; is accepted as the &#x60;ip&#x60; half. | [optional] 
**HealthCheck** | Pointer to [**NullableHealthCheckRequest**](HealthCheckRequest.md) | Optional health check. When omitted, the daemon installs a no-op placeholder (&#x60;HealthCheck::Tcp { port: 0 }&#x60;) matching the current default; the health monitor treats &#x60;port &#x3D;&#x3D; 0&#x60; as \&quot;skip\&quot;. | [optional] 
**Hostname** | Pointer to **NullableString** | Optional container hostname (maps to Docker&#39;s &#x60;--hostname&#x60;). | [optional] 
**Image** | **string** | OCI image reference (e.g., \&quot;nginx:latest\&quot;, \&quot;ubuntu:22.04\&quot;) | 
**InitContainer** | Pointer to **NullableBool** | Run a Docker-supplied init process (PID 1) inside the container (Docker &#x60;--init&#x60;). Distinct from &#x60;ZLayer&#x60;&#39;s pre-start init actions. | [optional] 
**IpcMode** | Pointer to **NullableString** | IPC namespace mode (Docker &#x60;--ipc&#x60;). Accepts e.g. &#x60;\&quot;host\&quot;&#x60;, &#x60;\&quot;shareable\&quot;&#x60;, &#x60;\&quot;private\&quot;&#x60;, or &#x60;\&quot;container:&lt;id&gt;\&quot;&#x60;. | [optional] 
**Labels** | Pointer to **map[string]string** | Labels for filtering and grouping | [optional] 
**Lifecycle** | Pointer to [**LifecycleSpec**](LifecycleSpec.md) | Container lifecycle policy. Carries the &#x60;delete_on_exit&#x60; knob (Docker &#x60;--rm&#x60; / &#x60;HostConfig.AutoRemove&#x60;) so the daemon can remove terminated container records and bundles once they exit. Defaults to [&#x60;crate::spec::LifecycleSpec::default()&#x60;] (i.e. retain on exit), which matches the historical behavior for callers that omit the field. | [optional] 
**MemoryReservation** | Pointer to **NullableString** | Soft memory limit (Docker &#x60;--memory-reservation&#x60;). | [optional] 
**MemorySwap** | Pointer to **NullableString** | Total memory limit including swap (Docker &#x60;--memory-swap&#x60;). | [optional] 
**MemorySwappiness** | Pointer to **NullableInt32** | Container memory swappiness, 0-100 (Docker &#x60;--memory-swappiness&#x60;). | [optional] 
**Name** | Pointer to **NullableString** | Optional human-readable name | [optional] 
**NetworkMode** | Pointer to [**NullableNetworkMode**](NetworkMode.md) | Network mode (Docker &#x60;--network&#x60;). Accepts &#x60;\&quot;default\&quot;&#x60;, &#x60;\&quot;host\&quot;&#x60;, &#x60;\&quot;none\&quot;&#x60;, &#x60;\&quot;bridge\&quot;&#x60;, &#x60;\&quot;bridge:&lt;name&gt;\&quot;&#x60;, or &#x60;\&quot;container:&lt;id&gt;\&quot;&#x60;. When omitted, defaults to [&#x60;crate::spec::NetworkMode::Default&#x60;]. | [optional] 
**Networks** | Pointer to [**[]NetworkAttachmentRequest**](NetworkAttachmentRequest.md) | User-defined bridge/overlay networks to attach the newly-created container to. Each entry references a network by id or name and is attached after the container is successfully started. If any attachment fails, the partially-started container is rolled back (stopped + removed) and the request is failed. | [optional] 
**NodeSelector** | Pointer to [**NullableNodeSelector**](NodeSelector.md) | Node selection constraints (required / preferred labels). When set on a daemon that has a cluster handle, the leader places the container on a node whose labels satisfy the required set; otherwise the field is ignored and the container is created locally. | [optional] 
**OomKillDisable** | Pointer to **NullableBool** | Disable the OOM killer for the container (Docker &#x60;--oom-kill-disable&#x60;). | [optional] 
**OomScoreAdj** | Pointer to **NullableInt32** | OOM-killer score adjustment (Docker &#x60;--oom-score-adj&#x60;). | [optional] 
**PidMode** | Pointer to **NullableString** | PID namespace mode (Docker &#x60;--pid&#x60;). Accepts e.g. &#x60;\&quot;host\&quot;&#x60; or &#x60;\&quot;container:&lt;id&gt;\&quot;&#x60;. | [optional] 
**PidsLimit** | Pointer to **NullableInt64** | Maximum number of processes the container may spawn (Docker &#x60;--pids-limit&#x60;). | [optional] 
**Platform** | Pointer to [**NullableTargetPlatform**](TargetPlatform.md) | Target platform (OS + arch) the container must run on, e.g. &#x60;darwin/arm64&#x60;. When set on a clustered daemon, the leader places the container on a node whose reported platform matches; when no node matches, the request is rejected. Ignored on single-node daemons. | [optional] 
**Ports** | Pointer to [**[]PortMapping**](PortMapping.md) | Published ports (Docker&#39;s &#x60;-p host:container/proto&#x60;). When omitted, the container is created without any host port publishing. | [optional] 
**Privileged** | Pointer to **NullableBool** | Run the container in privileged mode (Docker &#x60;--privileged&#x60;). When omitted, defaults to &#x60;false&#x60;. | [optional] 
**PullPolicy** | Pointer to **NullableString** | Image pull policy: \&quot;always\&quot;, \&quot;&#x60;if_not_present&#x60;\&quot;, or \&quot;never\&quot; | [optional] 
**ReadOnlyRootFs** | Pointer to **bool** | Mount the container&#39;s root filesystem read-only (Docker &#x60;--read-only&#x60;). | [optional] 
**RegistryAuth** | Pointer to [**NullableRegistryAuth**](RegistryAuth.md) | Inline Docker/OCI registry credentials used for this pull only. Not persisted, never logged, never echoed back on a response. When both &#x60;registry_credential_id&#x60; and &#x60;registry_auth&#x60; are set, this field takes precedence. | [optional] 
**RegistryCredentialId** | Pointer to **NullableString** | Id of a persisted registry credential (from &#x60;POST /api/v1/credentials/registry&#x60;) to use when pulling the image. Ignored when [&#x60;Self::registry_auth&#x60;] is also supplied (inline auth wins). Requires the daemon to be configured with a credential store — otherwise the request is rejected with &#x60;400&#x60;. | [optional] 
**Resources** | Pointer to [**NullableContainerResourceLimits**](ContainerResourceLimits.md) | Resource limits (CPU, memory) | [optional] 
**RestartPolicy** | Pointer to [**NullableContainerRestartPolicy**](ContainerRestartPolicy.md) | Container restart policy (Docker-style). When omitted, the runtime applies no explicit restart policy (Docker default: &#x60;\&quot;no\&quot;&#x60;). | [optional] 
**SecurityOpt** | Pointer to **[]string** | Security options such as &#x60;apparmor&#x3D;...&#x60;, &#x60;seccomp&#x3D;...&#x60;, &#x60;no-new-privileges:true&#x60; (Docker &#x60;--security-opt&#x60;). | [optional] 
**StopGracePeriod** | Pointer to **NullableString** | Grace period to wait between the stop signal and a forced kill (Docker &#x60;--stop-timeout&#x60;). Wire format is a humantime string (e.g. &#x60;\&quot;30s\&quot;&#x60;, &#x60;\&quot;500ms\&quot;&#x60;, &#x60;\&quot;1m\&quot;&#x60;). | [optional] 
**StopSignal** | Pointer to **NullableString** | Signal sent to the container&#39;s main process to request a graceful shutdown (Docker &#x60;--stop-signal&#x60;). Accepts e.g. &#x60;\&quot;SIGTERM\&quot;&#x60; or &#x60;\&quot;15\&quot;&#x60;. | [optional] 
**Sysctls** | Pointer to **map[string]string** | Kernel sysctl overrides (Docker &#x60;--sysctl&#x60;). | [optional] 
**Ulimits** | Pointer to [**map[string]UlimitSpec**](UlimitSpec.md) | Per-process ulimits (Docker &#x60;--ulimit&#x60;). | [optional] 
**User** | Pointer to **NullableString** | User and group override for the container&#39;s main process (Docker &#x60;--user uid:gid&#x60;). | [optional] 
**Volumes** | Pointer to [**[]VolumeMount**](VolumeMount.md) | Volume mounts | [optional] 
**WorkDir** | Pointer to **NullableString** | Working directory inside the container | [optional] 

## Methods

### NewCreateContainerRequest

`func NewCreateContainerRequest(image string, ) *CreateContainerRequest`

NewCreateContainerRequest instantiates a new CreateContainerRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateContainerRequestWithDefaults

`func NewCreateContainerRequestWithDefaults() *CreateContainerRequest`

NewCreateContainerRequestWithDefaults instantiates a new CreateContainerRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetBlkioWeight

`func (o *CreateContainerRequest) GetBlkioWeight() int32`

GetBlkioWeight returns the BlkioWeight field if non-nil, zero value otherwise.

### GetBlkioWeightOk

`func (o *CreateContainerRequest) GetBlkioWeightOk() (*int32, bool)`

GetBlkioWeightOk returns a tuple with the BlkioWeight field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBlkioWeight

`func (o *CreateContainerRequest) SetBlkioWeight(v int32)`

SetBlkioWeight sets BlkioWeight field to given value.

### HasBlkioWeight

`func (o *CreateContainerRequest) HasBlkioWeight() bool`

HasBlkioWeight returns a boolean if a field has been set.

### SetBlkioWeightNil

`func (o *CreateContainerRequest) SetBlkioWeightNil(b bool)`

 SetBlkioWeightNil sets the value for BlkioWeight to be an explicit nil

### UnsetBlkioWeight
`func (o *CreateContainerRequest) UnsetBlkioWeight()`

UnsetBlkioWeight ensures that no value is present for BlkioWeight, not even an explicit nil
### GetCapAdd

`func (o *CreateContainerRequest) GetCapAdd() []string`

GetCapAdd returns the CapAdd field if non-nil, zero value otherwise.

### GetCapAddOk

`func (o *CreateContainerRequest) GetCapAddOk() (*[]string, bool)`

GetCapAddOk returns a tuple with the CapAdd field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCapAdd

`func (o *CreateContainerRequest) SetCapAdd(v []string)`

SetCapAdd sets CapAdd field to given value.

### HasCapAdd

`func (o *CreateContainerRequest) HasCapAdd() bool`

HasCapAdd returns a boolean if a field has been set.

### GetCapDrop

`func (o *CreateContainerRequest) GetCapDrop() []string`

GetCapDrop returns the CapDrop field if non-nil, zero value otherwise.

### GetCapDropOk

`func (o *CreateContainerRequest) GetCapDropOk() (*[]string, bool)`

GetCapDropOk returns a tuple with the CapDrop field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCapDrop

`func (o *CreateContainerRequest) SetCapDrop(v []string)`

SetCapDrop sets CapDrop field to given value.

### HasCapDrop

`func (o *CreateContainerRequest) HasCapDrop() bool`

HasCapDrop returns a boolean if a field has been set.

### GetCommand

`func (o *CreateContainerRequest) GetCommand() []string`

GetCommand returns the Command field if non-nil, zero value otherwise.

### GetCommandOk

`func (o *CreateContainerRequest) GetCommandOk() (*[]string, bool)`

GetCommandOk returns a tuple with the Command field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCommand

`func (o *CreateContainerRequest) SetCommand(v []string)`

SetCommand sets Command field to given value.

### HasCommand

`func (o *CreateContainerRequest) HasCommand() bool`

HasCommand returns a boolean if a field has been set.

### SetCommandNil

`func (o *CreateContainerRequest) SetCommandNil(b bool)`

 SetCommandNil sets the value for Command to be an explicit nil

### UnsetCommand
`func (o *CreateContainerRequest) UnsetCommand()`

UnsetCommand ensures that no value is present for Command, not even an explicit nil
### GetCpuShares

`func (o *CreateContainerRequest) GetCpuShares() int32`

GetCpuShares returns the CpuShares field if non-nil, zero value otherwise.

### GetCpuSharesOk

`func (o *CreateContainerRequest) GetCpuSharesOk() (*int32, bool)`

GetCpuSharesOk returns a tuple with the CpuShares field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpuShares

`func (o *CreateContainerRequest) SetCpuShares(v int32)`

SetCpuShares sets CpuShares field to given value.

### HasCpuShares

`func (o *CreateContainerRequest) HasCpuShares() bool`

HasCpuShares returns a boolean if a field has been set.

### SetCpuSharesNil

`func (o *CreateContainerRequest) SetCpuSharesNil(b bool)`

 SetCpuSharesNil sets the value for CpuShares to be an explicit nil

### UnsetCpuShares
`func (o *CreateContainerRequest) UnsetCpuShares()`

UnsetCpuShares ensures that no value is present for CpuShares, not even an explicit nil
### GetCpuset

`func (o *CreateContainerRequest) GetCpuset() string`

GetCpuset returns the Cpuset field if non-nil, zero value otherwise.

### GetCpusetOk

`func (o *CreateContainerRequest) GetCpusetOk() (*string, bool)`

GetCpusetOk returns a tuple with the Cpuset field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpuset

`func (o *CreateContainerRequest) SetCpuset(v string)`

SetCpuset sets Cpuset field to given value.

### HasCpuset

`func (o *CreateContainerRequest) HasCpuset() bool`

HasCpuset returns a boolean if a field has been set.

### SetCpusetNil

`func (o *CreateContainerRequest) SetCpusetNil(b bool)`

 SetCpusetNil sets the value for Cpuset to be an explicit nil

### UnsetCpuset
`func (o *CreateContainerRequest) UnsetCpuset()`

UnsetCpuset ensures that no value is present for Cpuset, not even an explicit nil
### GetDevices

`func (o *CreateContainerRequest) GetDevices() []DeviceSpec`

GetDevices returns the Devices field if non-nil, zero value otherwise.

### GetDevicesOk

`func (o *CreateContainerRequest) GetDevicesOk() (*[]DeviceSpec, bool)`

GetDevicesOk returns a tuple with the Devices field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDevices

`func (o *CreateContainerRequest) SetDevices(v []DeviceSpec)`

SetDevices sets Devices field to given value.

### HasDevices

`func (o *CreateContainerRequest) HasDevices() bool`

HasDevices returns a boolean if a field has been set.

### GetDns

`func (o *CreateContainerRequest) GetDns() []string`

GetDns returns the Dns field if non-nil, zero value otherwise.

### GetDnsOk

`func (o *CreateContainerRequest) GetDnsOk() (*[]string, bool)`

GetDnsOk returns a tuple with the Dns field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDns

`func (o *CreateContainerRequest) SetDns(v []string)`

SetDns sets Dns field to given value.

### HasDns

`func (o *CreateContainerRequest) HasDns() bool`

HasDns returns a boolean if a field has been set.

### GetEnv

`func (o *CreateContainerRequest) GetEnv() map[string]string`

GetEnv returns the Env field if non-nil, zero value otherwise.

### GetEnvOk

`func (o *CreateContainerRequest) GetEnvOk() (*map[string]string, bool)`

GetEnvOk returns a tuple with the Env field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetEnv

`func (o *CreateContainerRequest) SetEnv(v map[string]string)`

SetEnv sets Env field to given value.

### HasEnv

`func (o *CreateContainerRequest) HasEnv() bool`

HasEnv returns a boolean if a field has been set.

### GetExtraGroups

`func (o *CreateContainerRequest) GetExtraGroups() []string`

GetExtraGroups returns the ExtraGroups field if non-nil, zero value otherwise.

### GetExtraGroupsOk

`func (o *CreateContainerRequest) GetExtraGroupsOk() (*[]string, bool)`

GetExtraGroupsOk returns a tuple with the ExtraGroups field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExtraGroups

`func (o *CreateContainerRequest) SetExtraGroups(v []string)`

SetExtraGroups sets ExtraGroups field to given value.

### HasExtraGroups

`func (o *CreateContainerRequest) HasExtraGroups() bool`

HasExtraGroups returns a boolean if a field has been set.

### GetExtraHosts

`func (o *CreateContainerRequest) GetExtraHosts() []string`

GetExtraHosts returns the ExtraHosts field if non-nil, zero value otherwise.

### GetExtraHostsOk

`func (o *CreateContainerRequest) GetExtraHostsOk() (*[]string, bool)`

GetExtraHostsOk returns a tuple with the ExtraHosts field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExtraHosts

`func (o *CreateContainerRequest) SetExtraHosts(v []string)`

SetExtraHosts sets ExtraHosts field to given value.

### HasExtraHosts

`func (o *CreateContainerRequest) HasExtraHosts() bool`

HasExtraHosts returns a boolean if a field has been set.

### GetHealthCheck

`func (o *CreateContainerRequest) GetHealthCheck() HealthCheckRequest`

GetHealthCheck returns the HealthCheck field if non-nil, zero value otherwise.

### GetHealthCheckOk

`func (o *CreateContainerRequest) GetHealthCheckOk() (*HealthCheckRequest, bool)`

GetHealthCheckOk returns a tuple with the HealthCheck field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHealthCheck

`func (o *CreateContainerRequest) SetHealthCheck(v HealthCheckRequest)`

SetHealthCheck sets HealthCheck field to given value.

### HasHealthCheck

`func (o *CreateContainerRequest) HasHealthCheck() bool`

HasHealthCheck returns a boolean if a field has been set.

### SetHealthCheckNil

`func (o *CreateContainerRequest) SetHealthCheckNil(b bool)`

 SetHealthCheckNil sets the value for HealthCheck to be an explicit nil

### UnsetHealthCheck
`func (o *CreateContainerRequest) UnsetHealthCheck()`

UnsetHealthCheck ensures that no value is present for HealthCheck, not even an explicit nil
### GetHostname

`func (o *CreateContainerRequest) GetHostname() string`

GetHostname returns the Hostname field if non-nil, zero value otherwise.

### GetHostnameOk

`func (o *CreateContainerRequest) GetHostnameOk() (*string, bool)`

GetHostnameOk returns a tuple with the Hostname field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHostname

`func (o *CreateContainerRequest) SetHostname(v string)`

SetHostname sets Hostname field to given value.

### HasHostname

`func (o *CreateContainerRequest) HasHostname() bool`

HasHostname returns a boolean if a field has been set.

### SetHostnameNil

`func (o *CreateContainerRequest) SetHostnameNil(b bool)`

 SetHostnameNil sets the value for Hostname to be an explicit nil

### UnsetHostname
`func (o *CreateContainerRequest) UnsetHostname()`

UnsetHostname ensures that no value is present for Hostname, not even an explicit nil
### GetImage

`func (o *CreateContainerRequest) GetImage() string`

GetImage returns the Image field if non-nil, zero value otherwise.

### GetImageOk

`func (o *CreateContainerRequest) GetImageOk() (*string, bool)`

GetImageOk returns a tuple with the Image field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetImage

`func (o *CreateContainerRequest) SetImage(v string)`

SetImage sets Image field to given value.


### GetInitContainer

`func (o *CreateContainerRequest) GetInitContainer() bool`

GetInitContainer returns the InitContainer field if non-nil, zero value otherwise.

### GetInitContainerOk

`func (o *CreateContainerRequest) GetInitContainerOk() (*bool, bool)`

GetInitContainerOk returns a tuple with the InitContainer field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetInitContainer

`func (o *CreateContainerRequest) SetInitContainer(v bool)`

SetInitContainer sets InitContainer field to given value.

### HasInitContainer

`func (o *CreateContainerRequest) HasInitContainer() bool`

HasInitContainer returns a boolean if a field has been set.

### SetInitContainerNil

`func (o *CreateContainerRequest) SetInitContainerNil(b bool)`

 SetInitContainerNil sets the value for InitContainer to be an explicit nil

### UnsetInitContainer
`func (o *CreateContainerRequest) UnsetInitContainer()`

UnsetInitContainer ensures that no value is present for InitContainer, not even an explicit nil
### GetIpcMode

`func (o *CreateContainerRequest) GetIpcMode() string`

GetIpcMode returns the IpcMode field if non-nil, zero value otherwise.

### GetIpcModeOk

`func (o *CreateContainerRequest) GetIpcModeOk() (*string, bool)`

GetIpcModeOk returns a tuple with the IpcMode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetIpcMode

`func (o *CreateContainerRequest) SetIpcMode(v string)`

SetIpcMode sets IpcMode field to given value.

### HasIpcMode

`func (o *CreateContainerRequest) HasIpcMode() bool`

HasIpcMode returns a boolean if a field has been set.

### SetIpcModeNil

`func (o *CreateContainerRequest) SetIpcModeNil(b bool)`

 SetIpcModeNil sets the value for IpcMode to be an explicit nil

### UnsetIpcMode
`func (o *CreateContainerRequest) UnsetIpcMode()`

UnsetIpcMode ensures that no value is present for IpcMode, not even an explicit nil
### GetLabels

`func (o *CreateContainerRequest) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *CreateContainerRequest) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *CreateContainerRequest) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.

### HasLabels

`func (o *CreateContainerRequest) HasLabels() bool`

HasLabels returns a boolean if a field has been set.

### GetLifecycle

`func (o *CreateContainerRequest) GetLifecycle() LifecycleSpec`

GetLifecycle returns the Lifecycle field if non-nil, zero value otherwise.

### GetLifecycleOk

`func (o *CreateContainerRequest) GetLifecycleOk() (*LifecycleSpec, bool)`

GetLifecycleOk returns a tuple with the Lifecycle field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLifecycle

`func (o *CreateContainerRequest) SetLifecycle(v LifecycleSpec)`

SetLifecycle sets Lifecycle field to given value.

### HasLifecycle

`func (o *CreateContainerRequest) HasLifecycle() bool`

HasLifecycle returns a boolean if a field has been set.

### GetMemoryReservation

`func (o *CreateContainerRequest) GetMemoryReservation() string`

GetMemoryReservation returns the MemoryReservation field if non-nil, zero value otherwise.

### GetMemoryReservationOk

`func (o *CreateContainerRequest) GetMemoryReservationOk() (*string, bool)`

GetMemoryReservationOk returns a tuple with the MemoryReservation field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemoryReservation

`func (o *CreateContainerRequest) SetMemoryReservation(v string)`

SetMemoryReservation sets MemoryReservation field to given value.

### HasMemoryReservation

`func (o *CreateContainerRequest) HasMemoryReservation() bool`

HasMemoryReservation returns a boolean if a field has been set.

### SetMemoryReservationNil

`func (o *CreateContainerRequest) SetMemoryReservationNil(b bool)`

 SetMemoryReservationNil sets the value for MemoryReservation to be an explicit nil

### UnsetMemoryReservation
`func (o *CreateContainerRequest) UnsetMemoryReservation()`

UnsetMemoryReservation ensures that no value is present for MemoryReservation, not even an explicit nil
### GetMemorySwap

`func (o *CreateContainerRequest) GetMemorySwap() string`

GetMemorySwap returns the MemorySwap field if non-nil, zero value otherwise.

### GetMemorySwapOk

`func (o *CreateContainerRequest) GetMemorySwapOk() (*string, bool)`

GetMemorySwapOk returns a tuple with the MemorySwap field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemorySwap

`func (o *CreateContainerRequest) SetMemorySwap(v string)`

SetMemorySwap sets MemorySwap field to given value.

### HasMemorySwap

`func (o *CreateContainerRequest) HasMemorySwap() bool`

HasMemorySwap returns a boolean if a field has been set.

### SetMemorySwapNil

`func (o *CreateContainerRequest) SetMemorySwapNil(b bool)`

 SetMemorySwapNil sets the value for MemorySwap to be an explicit nil

### UnsetMemorySwap
`func (o *CreateContainerRequest) UnsetMemorySwap()`

UnsetMemorySwap ensures that no value is present for MemorySwap, not even an explicit nil
### GetMemorySwappiness

`func (o *CreateContainerRequest) GetMemorySwappiness() int32`

GetMemorySwappiness returns the MemorySwappiness field if non-nil, zero value otherwise.

### GetMemorySwappinessOk

`func (o *CreateContainerRequest) GetMemorySwappinessOk() (*int32, bool)`

GetMemorySwappinessOk returns a tuple with the MemorySwappiness field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemorySwappiness

`func (o *CreateContainerRequest) SetMemorySwappiness(v int32)`

SetMemorySwappiness sets MemorySwappiness field to given value.

### HasMemorySwappiness

`func (o *CreateContainerRequest) HasMemorySwappiness() bool`

HasMemorySwappiness returns a boolean if a field has been set.

### SetMemorySwappinessNil

`func (o *CreateContainerRequest) SetMemorySwappinessNil(b bool)`

 SetMemorySwappinessNil sets the value for MemorySwappiness to be an explicit nil

### UnsetMemorySwappiness
`func (o *CreateContainerRequest) UnsetMemorySwappiness()`

UnsetMemorySwappiness ensures that no value is present for MemorySwappiness, not even an explicit nil
### GetName

`func (o *CreateContainerRequest) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *CreateContainerRequest) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *CreateContainerRequest) SetName(v string)`

SetName sets Name field to given value.

### HasName

`func (o *CreateContainerRequest) HasName() bool`

HasName returns a boolean if a field has been set.

### SetNameNil

`func (o *CreateContainerRequest) SetNameNil(b bool)`

 SetNameNil sets the value for Name to be an explicit nil

### UnsetName
`func (o *CreateContainerRequest) UnsetName()`

UnsetName ensures that no value is present for Name, not even an explicit nil
### GetNetworkMode

`func (o *CreateContainerRequest) GetNetworkMode() NetworkMode`

GetNetworkMode returns the NetworkMode field if non-nil, zero value otherwise.

### GetNetworkModeOk

`func (o *CreateContainerRequest) GetNetworkModeOk() (*NetworkMode, bool)`

GetNetworkModeOk returns a tuple with the NetworkMode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNetworkMode

`func (o *CreateContainerRequest) SetNetworkMode(v NetworkMode)`

SetNetworkMode sets NetworkMode field to given value.

### HasNetworkMode

`func (o *CreateContainerRequest) HasNetworkMode() bool`

HasNetworkMode returns a boolean if a field has been set.

### SetNetworkModeNil

`func (o *CreateContainerRequest) SetNetworkModeNil(b bool)`

 SetNetworkModeNil sets the value for NetworkMode to be an explicit nil

### UnsetNetworkMode
`func (o *CreateContainerRequest) UnsetNetworkMode()`

UnsetNetworkMode ensures that no value is present for NetworkMode, not even an explicit nil
### GetNetworks

`func (o *CreateContainerRequest) GetNetworks() []NetworkAttachmentRequest`

GetNetworks returns the Networks field if non-nil, zero value otherwise.

### GetNetworksOk

`func (o *CreateContainerRequest) GetNetworksOk() (*[]NetworkAttachmentRequest, bool)`

GetNetworksOk returns a tuple with the Networks field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNetworks

`func (o *CreateContainerRequest) SetNetworks(v []NetworkAttachmentRequest)`

SetNetworks sets Networks field to given value.

### HasNetworks

`func (o *CreateContainerRequest) HasNetworks() bool`

HasNetworks returns a boolean if a field has been set.

### GetNodeSelector

`func (o *CreateContainerRequest) GetNodeSelector() NodeSelector`

GetNodeSelector returns the NodeSelector field if non-nil, zero value otherwise.

### GetNodeSelectorOk

`func (o *CreateContainerRequest) GetNodeSelectorOk() (*NodeSelector, bool)`

GetNodeSelectorOk returns a tuple with the NodeSelector field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNodeSelector

`func (o *CreateContainerRequest) SetNodeSelector(v NodeSelector)`

SetNodeSelector sets NodeSelector field to given value.

### HasNodeSelector

`func (o *CreateContainerRequest) HasNodeSelector() bool`

HasNodeSelector returns a boolean if a field has been set.

### SetNodeSelectorNil

`func (o *CreateContainerRequest) SetNodeSelectorNil(b bool)`

 SetNodeSelectorNil sets the value for NodeSelector to be an explicit nil

### UnsetNodeSelector
`func (o *CreateContainerRequest) UnsetNodeSelector()`

UnsetNodeSelector ensures that no value is present for NodeSelector, not even an explicit nil
### GetOomKillDisable

`func (o *CreateContainerRequest) GetOomKillDisable() bool`

GetOomKillDisable returns the OomKillDisable field if non-nil, zero value otherwise.

### GetOomKillDisableOk

`func (o *CreateContainerRequest) GetOomKillDisableOk() (*bool, bool)`

GetOomKillDisableOk returns a tuple with the OomKillDisable field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOomKillDisable

`func (o *CreateContainerRequest) SetOomKillDisable(v bool)`

SetOomKillDisable sets OomKillDisable field to given value.

### HasOomKillDisable

`func (o *CreateContainerRequest) HasOomKillDisable() bool`

HasOomKillDisable returns a boolean if a field has been set.

### SetOomKillDisableNil

`func (o *CreateContainerRequest) SetOomKillDisableNil(b bool)`

 SetOomKillDisableNil sets the value for OomKillDisable to be an explicit nil

### UnsetOomKillDisable
`func (o *CreateContainerRequest) UnsetOomKillDisable()`

UnsetOomKillDisable ensures that no value is present for OomKillDisable, not even an explicit nil
### GetOomScoreAdj

`func (o *CreateContainerRequest) GetOomScoreAdj() int32`

GetOomScoreAdj returns the OomScoreAdj field if non-nil, zero value otherwise.

### GetOomScoreAdjOk

`func (o *CreateContainerRequest) GetOomScoreAdjOk() (*int32, bool)`

GetOomScoreAdjOk returns a tuple with the OomScoreAdj field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOomScoreAdj

`func (o *CreateContainerRequest) SetOomScoreAdj(v int32)`

SetOomScoreAdj sets OomScoreAdj field to given value.

### HasOomScoreAdj

`func (o *CreateContainerRequest) HasOomScoreAdj() bool`

HasOomScoreAdj returns a boolean if a field has been set.

### SetOomScoreAdjNil

`func (o *CreateContainerRequest) SetOomScoreAdjNil(b bool)`

 SetOomScoreAdjNil sets the value for OomScoreAdj to be an explicit nil

### UnsetOomScoreAdj
`func (o *CreateContainerRequest) UnsetOomScoreAdj()`

UnsetOomScoreAdj ensures that no value is present for OomScoreAdj, not even an explicit nil
### GetPidMode

`func (o *CreateContainerRequest) GetPidMode() string`

GetPidMode returns the PidMode field if non-nil, zero value otherwise.

### GetPidModeOk

`func (o *CreateContainerRequest) GetPidModeOk() (*string, bool)`

GetPidModeOk returns a tuple with the PidMode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPidMode

`func (o *CreateContainerRequest) SetPidMode(v string)`

SetPidMode sets PidMode field to given value.

### HasPidMode

`func (o *CreateContainerRequest) HasPidMode() bool`

HasPidMode returns a boolean if a field has been set.

### SetPidModeNil

`func (o *CreateContainerRequest) SetPidModeNil(b bool)`

 SetPidModeNil sets the value for PidMode to be an explicit nil

### UnsetPidMode
`func (o *CreateContainerRequest) UnsetPidMode()`

UnsetPidMode ensures that no value is present for PidMode, not even an explicit nil
### GetPidsLimit

`func (o *CreateContainerRequest) GetPidsLimit() int64`

GetPidsLimit returns the PidsLimit field if non-nil, zero value otherwise.

### GetPidsLimitOk

`func (o *CreateContainerRequest) GetPidsLimitOk() (*int64, bool)`

GetPidsLimitOk returns a tuple with the PidsLimit field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPidsLimit

`func (o *CreateContainerRequest) SetPidsLimit(v int64)`

SetPidsLimit sets PidsLimit field to given value.

### HasPidsLimit

`func (o *CreateContainerRequest) HasPidsLimit() bool`

HasPidsLimit returns a boolean if a field has been set.

### SetPidsLimitNil

`func (o *CreateContainerRequest) SetPidsLimitNil(b bool)`

 SetPidsLimitNil sets the value for PidsLimit to be an explicit nil

### UnsetPidsLimit
`func (o *CreateContainerRequest) UnsetPidsLimit()`

UnsetPidsLimit ensures that no value is present for PidsLimit, not even an explicit nil
### GetPlatform

`func (o *CreateContainerRequest) GetPlatform() TargetPlatform`

GetPlatform returns the Platform field if non-nil, zero value otherwise.

### GetPlatformOk

`func (o *CreateContainerRequest) GetPlatformOk() (*TargetPlatform, bool)`

GetPlatformOk returns a tuple with the Platform field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPlatform

`func (o *CreateContainerRequest) SetPlatform(v TargetPlatform)`

SetPlatform sets Platform field to given value.

### HasPlatform

`func (o *CreateContainerRequest) HasPlatform() bool`

HasPlatform returns a boolean if a field has been set.

### SetPlatformNil

`func (o *CreateContainerRequest) SetPlatformNil(b bool)`

 SetPlatformNil sets the value for Platform to be an explicit nil

### UnsetPlatform
`func (o *CreateContainerRequest) UnsetPlatform()`

UnsetPlatform ensures that no value is present for Platform, not even an explicit nil
### GetPorts

`func (o *CreateContainerRequest) GetPorts() []PortMapping`

GetPorts returns the Ports field if non-nil, zero value otherwise.

### GetPortsOk

`func (o *CreateContainerRequest) GetPortsOk() (*[]PortMapping, bool)`

GetPortsOk returns a tuple with the Ports field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPorts

`func (o *CreateContainerRequest) SetPorts(v []PortMapping)`

SetPorts sets Ports field to given value.

### HasPorts

`func (o *CreateContainerRequest) HasPorts() bool`

HasPorts returns a boolean if a field has been set.

### GetPrivileged

`func (o *CreateContainerRequest) GetPrivileged() bool`

GetPrivileged returns the Privileged field if non-nil, zero value otherwise.

### GetPrivilegedOk

`func (o *CreateContainerRequest) GetPrivilegedOk() (*bool, bool)`

GetPrivilegedOk returns a tuple with the Privileged field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPrivileged

`func (o *CreateContainerRequest) SetPrivileged(v bool)`

SetPrivileged sets Privileged field to given value.

### HasPrivileged

`func (o *CreateContainerRequest) HasPrivileged() bool`

HasPrivileged returns a boolean if a field has been set.

### SetPrivilegedNil

`func (o *CreateContainerRequest) SetPrivilegedNil(b bool)`

 SetPrivilegedNil sets the value for Privileged to be an explicit nil

### UnsetPrivileged
`func (o *CreateContainerRequest) UnsetPrivileged()`

UnsetPrivileged ensures that no value is present for Privileged, not even an explicit nil
### GetPullPolicy

`func (o *CreateContainerRequest) GetPullPolicy() string`

GetPullPolicy returns the PullPolicy field if non-nil, zero value otherwise.

### GetPullPolicyOk

`func (o *CreateContainerRequest) GetPullPolicyOk() (*string, bool)`

GetPullPolicyOk returns a tuple with the PullPolicy field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPullPolicy

`func (o *CreateContainerRequest) SetPullPolicy(v string)`

SetPullPolicy sets PullPolicy field to given value.

### HasPullPolicy

`func (o *CreateContainerRequest) HasPullPolicy() bool`

HasPullPolicy returns a boolean if a field has been set.

### SetPullPolicyNil

`func (o *CreateContainerRequest) SetPullPolicyNil(b bool)`

 SetPullPolicyNil sets the value for PullPolicy to be an explicit nil

### UnsetPullPolicy
`func (o *CreateContainerRequest) UnsetPullPolicy()`

UnsetPullPolicy ensures that no value is present for PullPolicy, not even an explicit nil
### GetReadOnlyRootFs

`func (o *CreateContainerRequest) GetReadOnlyRootFs() bool`

GetReadOnlyRootFs returns the ReadOnlyRootFs field if non-nil, zero value otherwise.

### GetReadOnlyRootFsOk

`func (o *CreateContainerRequest) GetReadOnlyRootFsOk() (*bool, bool)`

GetReadOnlyRootFsOk returns a tuple with the ReadOnlyRootFs field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReadOnlyRootFs

`func (o *CreateContainerRequest) SetReadOnlyRootFs(v bool)`

SetReadOnlyRootFs sets ReadOnlyRootFs field to given value.

### HasReadOnlyRootFs

`func (o *CreateContainerRequest) HasReadOnlyRootFs() bool`

HasReadOnlyRootFs returns a boolean if a field has been set.

### GetRegistryAuth

`func (o *CreateContainerRequest) GetRegistryAuth() RegistryAuth`

GetRegistryAuth returns the RegistryAuth field if non-nil, zero value otherwise.

### GetRegistryAuthOk

`func (o *CreateContainerRequest) GetRegistryAuthOk() (*RegistryAuth, bool)`

GetRegistryAuthOk returns a tuple with the RegistryAuth field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRegistryAuth

`func (o *CreateContainerRequest) SetRegistryAuth(v RegistryAuth)`

SetRegistryAuth sets RegistryAuth field to given value.

### HasRegistryAuth

`func (o *CreateContainerRequest) HasRegistryAuth() bool`

HasRegistryAuth returns a boolean if a field has been set.

### SetRegistryAuthNil

`func (o *CreateContainerRequest) SetRegistryAuthNil(b bool)`

 SetRegistryAuthNil sets the value for RegistryAuth to be an explicit nil

### UnsetRegistryAuth
`func (o *CreateContainerRequest) UnsetRegistryAuth()`

UnsetRegistryAuth ensures that no value is present for RegistryAuth, not even an explicit nil
### GetRegistryCredentialId

`func (o *CreateContainerRequest) GetRegistryCredentialId() string`

GetRegistryCredentialId returns the RegistryCredentialId field if non-nil, zero value otherwise.

### GetRegistryCredentialIdOk

`func (o *CreateContainerRequest) GetRegistryCredentialIdOk() (*string, bool)`

GetRegistryCredentialIdOk returns a tuple with the RegistryCredentialId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRegistryCredentialId

`func (o *CreateContainerRequest) SetRegistryCredentialId(v string)`

SetRegistryCredentialId sets RegistryCredentialId field to given value.

### HasRegistryCredentialId

`func (o *CreateContainerRequest) HasRegistryCredentialId() bool`

HasRegistryCredentialId returns a boolean if a field has been set.

### SetRegistryCredentialIdNil

`func (o *CreateContainerRequest) SetRegistryCredentialIdNil(b bool)`

 SetRegistryCredentialIdNil sets the value for RegistryCredentialId to be an explicit nil

### UnsetRegistryCredentialId
`func (o *CreateContainerRequest) UnsetRegistryCredentialId()`

UnsetRegistryCredentialId ensures that no value is present for RegistryCredentialId, not even an explicit nil
### GetResources

`func (o *CreateContainerRequest) GetResources() ContainerResourceLimits`

GetResources returns the Resources field if non-nil, zero value otherwise.

### GetResourcesOk

`func (o *CreateContainerRequest) GetResourcesOk() (*ContainerResourceLimits, bool)`

GetResourcesOk returns a tuple with the Resources field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetResources

`func (o *CreateContainerRequest) SetResources(v ContainerResourceLimits)`

SetResources sets Resources field to given value.

### HasResources

`func (o *CreateContainerRequest) HasResources() bool`

HasResources returns a boolean if a field has been set.

### SetResourcesNil

`func (o *CreateContainerRequest) SetResourcesNil(b bool)`

 SetResourcesNil sets the value for Resources to be an explicit nil

### UnsetResources
`func (o *CreateContainerRequest) UnsetResources()`

UnsetResources ensures that no value is present for Resources, not even an explicit nil
### GetRestartPolicy

`func (o *CreateContainerRequest) GetRestartPolicy() ContainerRestartPolicy`

GetRestartPolicy returns the RestartPolicy field if non-nil, zero value otherwise.

### GetRestartPolicyOk

`func (o *CreateContainerRequest) GetRestartPolicyOk() (*ContainerRestartPolicy, bool)`

GetRestartPolicyOk returns a tuple with the RestartPolicy field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRestartPolicy

`func (o *CreateContainerRequest) SetRestartPolicy(v ContainerRestartPolicy)`

SetRestartPolicy sets RestartPolicy field to given value.

### HasRestartPolicy

`func (o *CreateContainerRequest) HasRestartPolicy() bool`

HasRestartPolicy returns a boolean if a field has been set.

### SetRestartPolicyNil

`func (o *CreateContainerRequest) SetRestartPolicyNil(b bool)`

 SetRestartPolicyNil sets the value for RestartPolicy to be an explicit nil

### UnsetRestartPolicy
`func (o *CreateContainerRequest) UnsetRestartPolicy()`

UnsetRestartPolicy ensures that no value is present for RestartPolicy, not even an explicit nil
### GetSecurityOpt

`func (o *CreateContainerRequest) GetSecurityOpt() []string`

GetSecurityOpt returns the SecurityOpt field if non-nil, zero value otherwise.

### GetSecurityOptOk

`func (o *CreateContainerRequest) GetSecurityOptOk() (*[]string, bool)`

GetSecurityOptOk returns a tuple with the SecurityOpt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSecurityOpt

`func (o *CreateContainerRequest) SetSecurityOpt(v []string)`

SetSecurityOpt sets SecurityOpt field to given value.

### HasSecurityOpt

`func (o *CreateContainerRequest) HasSecurityOpt() bool`

HasSecurityOpt returns a boolean if a field has been set.

### GetStopGracePeriod

`func (o *CreateContainerRequest) GetStopGracePeriod() string`

GetStopGracePeriod returns the StopGracePeriod field if non-nil, zero value otherwise.

### GetStopGracePeriodOk

`func (o *CreateContainerRequest) GetStopGracePeriodOk() (*string, bool)`

GetStopGracePeriodOk returns a tuple with the StopGracePeriod field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStopGracePeriod

`func (o *CreateContainerRequest) SetStopGracePeriod(v string)`

SetStopGracePeriod sets StopGracePeriod field to given value.

### HasStopGracePeriod

`func (o *CreateContainerRequest) HasStopGracePeriod() bool`

HasStopGracePeriod returns a boolean if a field has been set.

### SetStopGracePeriodNil

`func (o *CreateContainerRequest) SetStopGracePeriodNil(b bool)`

 SetStopGracePeriodNil sets the value for StopGracePeriod to be an explicit nil

### UnsetStopGracePeriod
`func (o *CreateContainerRequest) UnsetStopGracePeriod()`

UnsetStopGracePeriod ensures that no value is present for StopGracePeriod, not even an explicit nil
### GetStopSignal

`func (o *CreateContainerRequest) GetStopSignal() string`

GetStopSignal returns the StopSignal field if non-nil, zero value otherwise.

### GetStopSignalOk

`func (o *CreateContainerRequest) GetStopSignalOk() (*string, bool)`

GetStopSignalOk returns a tuple with the StopSignal field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStopSignal

`func (o *CreateContainerRequest) SetStopSignal(v string)`

SetStopSignal sets StopSignal field to given value.

### HasStopSignal

`func (o *CreateContainerRequest) HasStopSignal() bool`

HasStopSignal returns a boolean if a field has been set.

### SetStopSignalNil

`func (o *CreateContainerRequest) SetStopSignalNil(b bool)`

 SetStopSignalNil sets the value for StopSignal to be an explicit nil

### UnsetStopSignal
`func (o *CreateContainerRequest) UnsetStopSignal()`

UnsetStopSignal ensures that no value is present for StopSignal, not even an explicit nil
### GetSysctls

`func (o *CreateContainerRequest) GetSysctls() map[string]string`

GetSysctls returns the Sysctls field if non-nil, zero value otherwise.

### GetSysctlsOk

`func (o *CreateContainerRequest) GetSysctlsOk() (*map[string]string, bool)`

GetSysctlsOk returns a tuple with the Sysctls field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSysctls

`func (o *CreateContainerRequest) SetSysctls(v map[string]string)`

SetSysctls sets Sysctls field to given value.

### HasSysctls

`func (o *CreateContainerRequest) HasSysctls() bool`

HasSysctls returns a boolean if a field has been set.

### GetUlimits

`func (o *CreateContainerRequest) GetUlimits() map[string]UlimitSpec`

GetUlimits returns the Ulimits field if non-nil, zero value otherwise.

### GetUlimitsOk

`func (o *CreateContainerRequest) GetUlimitsOk() (*map[string]UlimitSpec, bool)`

GetUlimitsOk returns a tuple with the Ulimits field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUlimits

`func (o *CreateContainerRequest) SetUlimits(v map[string]UlimitSpec)`

SetUlimits sets Ulimits field to given value.

### HasUlimits

`func (o *CreateContainerRequest) HasUlimits() bool`

HasUlimits returns a boolean if a field has been set.

### GetUser

`func (o *CreateContainerRequest) GetUser() string`

GetUser returns the User field if non-nil, zero value otherwise.

### GetUserOk

`func (o *CreateContainerRequest) GetUserOk() (*string, bool)`

GetUserOk returns a tuple with the User field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUser

`func (o *CreateContainerRequest) SetUser(v string)`

SetUser sets User field to given value.

### HasUser

`func (o *CreateContainerRequest) HasUser() bool`

HasUser returns a boolean if a field has been set.

### SetUserNil

`func (o *CreateContainerRequest) SetUserNil(b bool)`

 SetUserNil sets the value for User to be an explicit nil

### UnsetUser
`func (o *CreateContainerRequest) UnsetUser()`

UnsetUser ensures that no value is present for User, not even an explicit nil
### GetVolumes

`func (o *CreateContainerRequest) GetVolumes() []VolumeMount`

GetVolumes returns the Volumes field if non-nil, zero value otherwise.

### GetVolumesOk

`func (o *CreateContainerRequest) GetVolumesOk() (*[]VolumeMount, bool)`

GetVolumesOk returns a tuple with the Volumes field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetVolumes

`func (o *CreateContainerRequest) SetVolumes(v []VolumeMount)`

SetVolumes sets Volumes field to given value.

### HasVolumes

`func (o *CreateContainerRequest) HasVolumes() bool`

HasVolumes returns a boolean if a field has been set.

### GetWorkDir

`func (o *CreateContainerRequest) GetWorkDir() string`

GetWorkDir returns the WorkDir field if non-nil, zero value otherwise.

### GetWorkDirOk

`func (o *CreateContainerRequest) GetWorkDirOk() (*string, bool)`

GetWorkDirOk returns a tuple with the WorkDir field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetWorkDir

`func (o *CreateContainerRequest) SetWorkDir(v string)`

SetWorkDir sets WorkDir field to given value.

### HasWorkDir

`func (o *CreateContainerRequest) HasWorkDir() bool`

HasWorkDir returns a boolean if a field has been set.

### SetWorkDirNil

`func (o *CreateContainerRequest) SetWorkDirNil(b bool)`

 SetWorkDirNil sets the value for WorkDir to be an explicit nil

### UnsetWorkDir
`func (o *CreateContainerRequest) UnsetWorkDir()`

UnsetWorkDir ensures that no value is present for WorkDir, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


