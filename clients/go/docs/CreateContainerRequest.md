# CreateContainerRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Command** | Pointer to **[]string** | Command to run (overrides image entrypoint) | [optional] 
**Dns** | Pointer to **[]string** | Additional DNS servers (maps to Docker&#39;s &#x60;--dns&#x60;). Each entry must be a plausible IPv4 or IPv6 address. | [optional] 
**Env** | Pointer to **map[string]string** | Environment variables | [optional] 
**ExtraHosts** | Pointer to **[]string** | Extra &#x60;hostname:ip&#x60; entries appended to &#x60;/etc/hosts&#x60; (maps to Docker&#39;s &#x60;--add-host&#x60;). The special literal &#x60;host-gateway&#x60; is accepted as the &#x60;ip&#x60; half. | [optional] 
**HealthCheck** | Pointer to [**NullableHealthCheckRequest**](HealthCheckRequest.md) | Optional health check. When omitted, the daemon installs a no-op placeholder (&#x60;HealthCheck::Tcp { port: 0 }&#x60;) matching the current default; the health monitor treats &#x60;port &#x3D;&#x3D; 0&#x60; as \&quot;skip\&quot;. | [optional] 
**Hostname** | Pointer to **NullableString** | Optional container hostname (maps to Docker&#39;s &#x60;--hostname&#x60;). | [optional] 
**Image** | **string** | OCI image reference (e.g., \&quot;nginx:latest\&quot;, \&quot;ubuntu:22.04\&quot;) | 
**Labels** | Pointer to **map[string]string** | Labels for filtering and grouping | [optional] 
**Name** | Pointer to **NullableString** | Optional human-readable name | [optional] 
**Networks** | Pointer to [**[]NetworkAttachmentRequest**](NetworkAttachmentRequest.md) | User-defined bridge/overlay networks to attach the newly-created container to. Each entry references a network by id or name and is attached after the container is successfully started. If any attachment fails, the partially-started container is rolled back (stopped + removed) and the request is failed. | [optional] 
**Ports** | Pointer to [**[]PortMapping**](PortMapping.md) | Published ports (Docker&#39;s &#x60;-p host:container/proto&#x60;). When omitted, the container is created without any host port publishing. | [optional] 
**PullPolicy** | Pointer to **NullableString** | Image pull policy: \&quot;always\&quot;, \&quot;&#x60;if_not_present&#x60;\&quot;, or \&quot;never\&quot; | [optional] 
**RegistryAuth** | Pointer to [**NullableRegistryAuth**](RegistryAuth.md) | Inline Docker/OCI registry credentials used for this pull only. Not persisted, never logged, never echoed back on a response. When both &#x60;registry_credential_id&#x60; and &#x60;registry_auth&#x60; are set, this field takes precedence. | [optional] 
**RegistryCredentialId** | Pointer to **NullableString** | Id of a persisted registry credential (from &#x60;POST /api/v1/credentials/registry&#x60;) to use when pulling the image. Ignored when [&#x60;Self::registry_auth&#x60;] is also supplied (inline auth wins). Requires the daemon to be configured with a credential store — otherwise the request is rejected with &#x60;400&#x60;. | [optional] 
**Resources** | Pointer to [**NullableContainerResourceLimits**](ContainerResourceLimits.md) | Resource limits (CPU, memory) | [optional] 
**RestartPolicy** | Pointer to [**NullableContainerRestartPolicy**](ContainerRestartPolicy.md) | Container restart policy (Docker-style). When omitted, the runtime applies no explicit restart policy (Docker default: &#x60;\&quot;no\&quot;&#x60;). | [optional] 
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


