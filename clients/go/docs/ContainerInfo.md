# ContainerInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CreatedAt** | **string** | Creation timestamp (ISO 8601) | 
**ExitCode** | Pointer to **NullableInt32** | Most-recent exit code. &#x60;None&#x60; for containers still running and for containers that have never exited. | [optional] 
**Health** | Pointer to [**NullableContainerHealthInfo**](ContainerHealthInfo.md) | Runtime-native health status, when the container image declares a &#x60;HEALTHCHECK&#x60; (or equivalent). &#x60;None&#x60; when the runtime doesn&#39;t track health for this container. | [optional] 
**Id** | **string** | Container identifier | 
**Image** | **string** | OCI image reference | 
**Ipv4** | Pointer to **NullableString** | Primary IPv4 address (first non-empty IP across attached networks). Docker&#39;s &#x60;bridge&#x60; network is preferred when present. | [optional] 
**Labels** | **map[string]string** | Labels | 
**Name** | Pointer to **NullableString** | Human-readable name (if set) | [optional] 
**Networks** | Pointer to [**[]NetworkAttachmentInfo**](NetworkAttachmentInfo.md) | Networks this container is attached to, with per-network aliases and IPv4. Empty when the runtime doesn&#39;t surface network detail. | [optional] 
**Pid** | Pointer to **NullableInt32** | Process ID (if running) | [optional] 
**Ports** | Pointer to [**[]PortMapping**](PortMapping.md) | Published port mappings (container → host). Populated from the runtime&#39;s inspect response; empty when the runtime doesn&#39;t expose port-level detail or the container has no published ports. | [optional] 
**State** | **string** | Container state (pending, running, exited, failed) | 

## Methods

### NewContainerInfo

`func NewContainerInfo(createdAt string, id string, image string, labels map[string]string, state string, ) *ContainerInfo`

NewContainerInfo instantiates a new ContainerInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerInfoWithDefaults

`func NewContainerInfoWithDefaults() *ContainerInfo`

NewContainerInfoWithDefaults instantiates a new ContainerInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCreatedAt

`func (o *ContainerInfo) GetCreatedAt() string`

GetCreatedAt returns the CreatedAt field if non-nil, zero value otherwise.

### GetCreatedAtOk

`func (o *ContainerInfo) GetCreatedAtOk() (*string, bool)`

GetCreatedAtOk returns a tuple with the CreatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreatedAt

`func (o *ContainerInfo) SetCreatedAt(v string)`

SetCreatedAt sets CreatedAt field to given value.


### GetExitCode

`func (o *ContainerInfo) GetExitCode() int32`

GetExitCode returns the ExitCode field if non-nil, zero value otherwise.

### GetExitCodeOk

`func (o *ContainerInfo) GetExitCodeOk() (*int32, bool)`

GetExitCodeOk returns a tuple with the ExitCode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExitCode

`func (o *ContainerInfo) SetExitCode(v int32)`

SetExitCode sets ExitCode field to given value.

### HasExitCode

`func (o *ContainerInfo) HasExitCode() bool`

HasExitCode returns a boolean if a field has been set.

### SetExitCodeNil

`func (o *ContainerInfo) SetExitCodeNil(b bool)`

 SetExitCodeNil sets the value for ExitCode to be an explicit nil

### UnsetExitCode
`func (o *ContainerInfo) UnsetExitCode()`

UnsetExitCode ensures that no value is present for ExitCode, not even an explicit nil
### GetHealth

`func (o *ContainerInfo) GetHealth() ContainerHealthInfo`

GetHealth returns the Health field if non-nil, zero value otherwise.

### GetHealthOk

`func (o *ContainerInfo) GetHealthOk() (*ContainerHealthInfo, bool)`

GetHealthOk returns a tuple with the Health field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHealth

`func (o *ContainerInfo) SetHealth(v ContainerHealthInfo)`

SetHealth sets Health field to given value.

### HasHealth

`func (o *ContainerInfo) HasHealth() bool`

HasHealth returns a boolean if a field has been set.

### SetHealthNil

`func (o *ContainerInfo) SetHealthNil(b bool)`

 SetHealthNil sets the value for Health to be an explicit nil

### UnsetHealth
`func (o *ContainerInfo) UnsetHealth()`

UnsetHealth ensures that no value is present for Health, not even an explicit nil
### GetId

`func (o *ContainerInfo) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *ContainerInfo) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *ContainerInfo) SetId(v string)`

SetId sets Id field to given value.


### GetImage

`func (o *ContainerInfo) GetImage() string`

GetImage returns the Image field if non-nil, zero value otherwise.

### GetImageOk

`func (o *ContainerInfo) GetImageOk() (*string, bool)`

GetImageOk returns a tuple with the Image field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetImage

`func (o *ContainerInfo) SetImage(v string)`

SetImage sets Image field to given value.


### GetIpv4

`func (o *ContainerInfo) GetIpv4() string`

GetIpv4 returns the Ipv4 field if non-nil, zero value otherwise.

### GetIpv4Ok

`func (o *ContainerInfo) GetIpv4Ok() (*string, bool)`

GetIpv4Ok returns a tuple with the Ipv4 field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetIpv4

`func (o *ContainerInfo) SetIpv4(v string)`

SetIpv4 sets Ipv4 field to given value.

### HasIpv4

`func (o *ContainerInfo) HasIpv4() bool`

HasIpv4 returns a boolean if a field has been set.

### SetIpv4Nil

`func (o *ContainerInfo) SetIpv4Nil(b bool)`

 SetIpv4Nil sets the value for Ipv4 to be an explicit nil

### UnsetIpv4
`func (o *ContainerInfo) UnsetIpv4()`

UnsetIpv4 ensures that no value is present for Ipv4, not even an explicit nil
### GetLabels

`func (o *ContainerInfo) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *ContainerInfo) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *ContainerInfo) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.


### GetName

`func (o *ContainerInfo) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *ContainerInfo) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *ContainerInfo) SetName(v string)`

SetName sets Name field to given value.

### HasName

`func (o *ContainerInfo) HasName() bool`

HasName returns a boolean if a field has been set.

### SetNameNil

`func (o *ContainerInfo) SetNameNil(b bool)`

 SetNameNil sets the value for Name to be an explicit nil

### UnsetName
`func (o *ContainerInfo) UnsetName()`

UnsetName ensures that no value is present for Name, not even an explicit nil
### GetNetworks

`func (o *ContainerInfo) GetNetworks() []NetworkAttachmentInfo`

GetNetworks returns the Networks field if non-nil, zero value otherwise.

### GetNetworksOk

`func (o *ContainerInfo) GetNetworksOk() (*[]NetworkAttachmentInfo, bool)`

GetNetworksOk returns a tuple with the Networks field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNetworks

`func (o *ContainerInfo) SetNetworks(v []NetworkAttachmentInfo)`

SetNetworks sets Networks field to given value.

### HasNetworks

`func (o *ContainerInfo) HasNetworks() bool`

HasNetworks returns a boolean if a field has been set.

### GetPid

`func (o *ContainerInfo) GetPid() int32`

GetPid returns the Pid field if non-nil, zero value otherwise.

### GetPidOk

`func (o *ContainerInfo) GetPidOk() (*int32, bool)`

GetPidOk returns a tuple with the Pid field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPid

`func (o *ContainerInfo) SetPid(v int32)`

SetPid sets Pid field to given value.

### HasPid

`func (o *ContainerInfo) HasPid() bool`

HasPid returns a boolean if a field has been set.

### SetPidNil

`func (o *ContainerInfo) SetPidNil(b bool)`

 SetPidNil sets the value for Pid to be an explicit nil

### UnsetPid
`func (o *ContainerInfo) UnsetPid()`

UnsetPid ensures that no value is present for Pid, not even an explicit nil
### GetPorts

`func (o *ContainerInfo) GetPorts() []PortMapping`

GetPorts returns the Ports field if non-nil, zero value otherwise.

### GetPortsOk

`func (o *ContainerInfo) GetPortsOk() (*[]PortMapping, bool)`

GetPortsOk returns a tuple with the Ports field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPorts

`func (o *ContainerInfo) SetPorts(v []PortMapping)`

SetPorts sets Ports field to given value.

### HasPorts

`func (o *ContainerInfo) HasPorts() bool`

HasPorts returns a boolean if a field has been set.

### GetState

`func (o *ContainerInfo) GetState() string`

GetState returns the State field if non-nil, zero value otherwise.

### GetStateOk

`func (o *ContainerInfo) GetStateOk() (*string, bool)`

GetStateOk returns a tuple with the State field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetState

`func (o *ContainerInfo) SetState(v string)`

SetState sets State field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


