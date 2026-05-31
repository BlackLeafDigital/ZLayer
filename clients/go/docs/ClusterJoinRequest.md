# ClusterJoinRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**AdvertiseAddr** | **string** | Joining node&#39;s advertise address (IP) | 
**ApiPort** | Pointer to **int32** | Joining node&#39;s API server port | [optional] 
**Arch** | Pointer to [**NullableArchKind**](ArchKind.md) | CPU architecture of the joining agent. Same legacy semantics as &#x60;os&#x60;. | [optional] 
**CpuTotal** | Pointer to **float64** | Total CPU cores on the joining node | [optional] 
**DiskTotal** | Pointer to **int64** | Total disk in bytes | [optional] 
**Gpus** | Pointer to [**[]GpuInfoSummary**](GpuInfoSummary.md) | Detected GPUs | [optional] 
**Labels** | Pointer to **map[string]string** | Free-form labels advertised by the joining agent, used for &#x60;NodeSelector&#x60; placement matching. Empty on legacy clients. | [optional] 
**MemoryTotal** | Pointer to **int64** | Total memory in bytes | [optional] 
**Mode** | Pointer to **string** | Node mode: \&quot;full\&quot; or \&quot;replicate\&quot; | [optional] 
**Os** | Pointer to [**NullableOsKind**](OsKind.md) | Operating system of the joining agent. &#x60;None&#x60; &#x3D; legacy client that did not report platform info. | [optional] 
**OverlayPort** | **int32** | Joining node&#39;s overlay port (&#x60;WireGuard&#x60;) | 
**RaftPort** | **int32** | Joining node&#39;s Raft RPC port | 
**SecretsPubkey** | Pointer to **[]int32** | Joiner&#39;s 32-byte X25519 pubkey for sealed-box DEK wrapping. Present on Phase-1+ joiners; absent on legacy clients (in which case the leader treats the node as not eligible to host replicated-secret ciphertext until it re-joins with a pubkey). | [optional] 
**Services** | Pointer to **[]string** | Services to replicate (only if mode &#x3D;&#x3D; \&quot;replicate\&quot;) | [optional] 
**Token** | **string** | Base64-encoded join token (contains &#x60;auth_secret&#x60; for validation) | 
**WgPublicKey** | **string** | Joining node&#39;s &#x60;WireGuard&#x60; public key | 

## Methods

### NewClusterJoinRequest

`func NewClusterJoinRequest(advertiseAddr string, overlayPort int32, raftPort int32, token string, wgPublicKey string, ) *ClusterJoinRequest`

NewClusterJoinRequest instantiates a new ClusterJoinRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewClusterJoinRequestWithDefaults

`func NewClusterJoinRequestWithDefaults() *ClusterJoinRequest`

NewClusterJoinRequestWithDefaults instantiates a new ClusterJoinRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAdvertiseAddr

`func (o *ClusterJoinRequest) GetAdvertiseAddr() string`

GetAdvertiseAddr returns the AdvertiseAddr field if non-nil, zero value otherwise.

### GetAdvertiseAddrOk

`func (o *ClusterJoinRequest) GetAdvertiseAddrOk() (*string, bool)`

GetAdvertiseAddrOk returns a tuple with the AdvertiseAddr field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAdvertiseAddr

`func (o *ClusterJoinRequest) SetAdvertiseAddr(v string)`

SetAdvertiseAddr sets AdvertiseAddr field to given value.


### GetApiPort

`func (o *ClusterJoinRequest) GetApiPort() int32`

GetApiPort returns the ApiPort field if non-nil, zero value otherwise.

### GetApiPortOk

`func (o *ClusterJoinRequest) GetApiPortOk() (*int32, bool)`

GetApiPortOk returns a tuple with the ApiPort field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetApiPort

`func (o *ClusterJoinRequest) SetApiPort(v int32)`

SetApiPort sets ApiPort field to given value.

### HasApiPort

`func (o *ClusterJoinRequest) HasApiPort() bool`

HasApiPort returns a boolean if a field has been set.

### GetArch

`func (o *ClusterJoinRequest) GetArch() ArchKind`

GetArch returns the Arch field if non-nil, zero value otherwise.

### GetArchOk

`func (o *ClusterJoinRequest) GetArchOk() (*ArchKind, bool)`

GetArchOk returns a tuple with the Arch field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetArch

`func (o *ClusterJoinRequest) SetArch(v ArchKind)`

SetArch sets Arch field to given value.

### HasArch

`func (o *ClusterJoinRequest) HasArch() bool`

HasArch returns a boolean if a field has been set.

### SetArchNil

`func (o *ClusterJoinRequest) SetArchNil(b bool)`

 SetArchNil sets the value for Arch to be an explicit nil

### UnsetArch
`func (o *ClusterJoinRequest) UnsetArch()`

UnsetArch ensures that no value is present for Arch, not even an explicit nil
### GetCpuTotal

`func (o *ClusterJoinRequest) GetCpuTotal() float64`

GetCpuTotal returns the CpuTotal field if non-nil, zero value otherwise.

### GetCpuTotalOk

`func (o *ClusterJoinRequest) GetCpuTotalOk() (*float64, bool)`

GetCpuTotalOk returns a tuple with the CpuTotal field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpuTotal

`func (o *ClusterJoinRequest) SetCpuTotal(v float64)`

SetCpuTotal sets CpuTotal field to given value.

### HasCpuTotal

`func (o *ClusterJoinRequest) HasCpuTotal() bool`

HasCpuTotal returns a boolean if a field has been set.

### GetDiskTotal

`func (o *ClusterJoinRequest) GetDiskTotal() int64`

GetDiskTotal returns the DiskTotal field if non-nil, zero value otherwise.

### GetDiskTotalOk

`func (o *ClusterJoinRequest) GetDiskTotalOk() (*int64, bool)`

GetDiskTotalOk returns a tuple with the DiskTotal field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDiskTotal

`func (o *ClusterJoinRequest) SetDiskTotal(v int64)`

SetDiskTotal sets DiskTotal field to given value.

### HasDiskTotal

`func (o *ClusterJoinRequest) HasDiskTotal() bool`

HasDiskTotal returns a boolean if a field has been set.

### GetGpus

`func (o *ClusterJoinRequest) GetGpus() []GpuInfoSummary`

GetGpus returns the Gpus field if non-nil, zero value otherwise.

### GetGpusOk

`func (o *ClusterJoinRequest) GetGpusOk() (*[]GpuInfoSummary, bool)`

GetGpusOk returns a tuple with the Gpus field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGpus

`func (o *ClusterJoinRequest) SetGpus(v []GpuInfoSummary)`

SetGpus sets Gpus field to given value.

### HasGpus

`func (o *ClusterJoinRequest) HasGpus() bool`

HasGpus returns a boolean if a field has been set.

### GetLabels

`func (o *ClusterJoinRequest) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *ClusterJoinRequest) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *ClusterJoinRequest) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.

### HasLabels

`func (o *ClusterJoinRequest) HasLabels() bool`

HasLabels returns a boolean if a field has been set.

### GetMemoryTotal

`func (o *ClusterJoinRequest) GetMemoryTotal() int64`

GetMemoryTotal returns the MemoryTotal field if non-nil, zero value otherwise.

### GetMemoryTotalOk

`func (o *ClusterJoinRequest) GetMemoryTotalOk() (*int64, bool)`

GetMemoryTotalOk returns a tuple with the MemoryTotal field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemoryTotal

`func (o *ClusterJoinRequest) SetMemoryTotal(v int64)`

SetMemoryTotal sets MemoryTotal field to given value.

### HasMemoryTotal

`func (o *ClusterJoinRequest) HasMemoryTotal() bool`

HasMemoryTotal returns a boolean if a field has been set.

### GetMode

`func (o *ClusterJoinRequest) GetMode() string`

GetMode returns the Mode field if non-nil, zero value otherwise.

### GetModeOk

`func (o *ClusterJoinRequest) GetModeOk() (*string, bool)`

GetModeOk returns a tuple with the Mode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMode

`func (o *ClusterJoinRequest) SetMode(v string)`

SetMode sets Mode field to given value.

### HasMode

`func (o *ClusterJoinRequest) HasMode() bool`

HasMode returns a boolean if a field has been set.

### GetOs

`func (o *ClusterJoinRequest) GetOs() OsKind`

GetOs returns the Os field if non-nil, zero value otherwise.

### GetOsOk

`func (o *ClusterJoinRequest) GetOsOk() (*OsKind, bool)`

GetOsOk returns a tuple with the Os field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOs

`func (o *ClusterJoinRequest) SetOs(v OsKind)`

SetOs sets Os field to given value.

### HasOs

`func (o *ClusterJoinRequest) HasOs() bool`

HasOs returns a boolean if a field has been set.

### SetOsNil

`func (o *ClusterJoinRequest) SetOsNil(b bool)`

 SetOsNil sets the value for Os to be an explicit nil

### UnsetOs
`func (o *ClusterJoinRequest) UnsetOs()`

UnsetOs ensures that no value is present for Os, not even an explicit nil
### GetOverlayPort

`func (o *ClusterJoinRequest) GetOverlayPort() int32`

GetOverlayPort returns the OverlayPort field if non-nil, zero value otherwise.

### GetOverlayPortOk

`func (o *ClusterJoinRequest) GetOverlayPortOk() (*int32, bool)`

GetOverlayPortOk returns a tuple with the OverlayPort field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOverlayPort

`func (o *ClusterJoinRequest) SetOverlayPort(v int32)`

SetOverlayPort sets OverlayPort field to given value.


### GetRaftPort

`func (o *ClusterJoinRequest) GetRaftPort() int32`

GetRaftPort returns the RaftPort field if non-nil, zero value otherwise.

### GetRaftPortOk

`func (o *ClusterJoinRequest) GetRaftPortOk() (*int32, bool)`

GetRaftPortOk returns a tuple with the RaftPort field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRaftPort

`func (o *ClusterJoinRequest) SetRaftPort(v int32)`

SetRaftPort sets RaftPort field to given value.


### GetSecretsPubkey

`func (o *ClusterJoinRequest) GetSecretsPubkey() []int32`

GetSecretsPubkey returns the SecretsPubkey field if non-nil, zero value otherwise.

### GetSecretsPubkeyOk

`func (o *ClusterJoinRequest) GetSecretsPubkeyOk() (*[]int32, bool)`

GetSecretsPubkeyOk returns a tuple with the SecretsPubkey field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSecretsPubkey

`func (o *ClusterJoinRequest) SetSecretsPubkey(v []int32)`

SetSecretsPubkey sets SecretsPubkey field to given value.

### HasSecretsPubkey

`func (o *ClusterJoinRequest) HasSecretsPubkey() bool`

HasSecretsPubkey returns a boolean if a field has been set.

### SetSecretsPubkeyNil

`func (o *ClusterJoinRequest) SetSecretsPubkeyNil(b bool)`

 SetSecretsPubkeyNil sets the value for SecretsPubkey to be an explicit nil

### UnsetSecretsPubkey
`func (o *ClusterJoinRequest) UnsetSecretsPubkey()`

UnsetSecretsPubkey ensures that no value is present for SecretsPubkey, not even an explicit nil
### GetServices

`func (o *ClusterJoinRequest) GetServices() []string`

GetServices returns the Services field if non-nil, zero value otherwise.

### GetServicesOk

`func (o *ClusterJoinRequest) GetServicesOk() (*[]string, bool)`

GetServicesOk returns a tuple with the Services field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetServices

`func (o *ClusterJoinRequest) SetServices(v []string)`

SetServices sets Services field to given value.

### HasServices

`func (o *ClusterJoinRequest) HasServices() bool`

HasServices returns a boolean if a field has been set.

### SetServicesNil

`func (o *ClusterJoinRequest) SetServicesNil(b bool)`

 SetServicesNil sets the value for Services to be an explicit nil

### UnsetServices
`func (o *ClusterJoinRequest) UnsetServices()`

UnsetServices ensures that no value is present for Services, not even an explicit nil
### GetToken

`func (o *ClusterJoinRequest) GetToken() string`

GetToken returns the Token field if non-nil, zero value otherwise.

### GetTokenOk

`func (o *ClusterJoinRequest) GetTokenOk() (*string, bool)`

GetTokenOk returns a tuple with the Token field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetToken

`func (o *ClusterJoinRequest) SetToken(v string)`

SetToken sets Token field to given value.


### GetWgPublicKey

`func (o *ClusterJoinRequest) GetWgPublicKey() string`

GetWgPublicKey returns the WgPublicKey field if non-nil, zero value otherwise.

### GetWgPublicKeyOk

`func (o *ClusterJoinRequest) GetWgPublicKeyOk() (*string, bool)`

GetWgPublicKeyOk returns a tuple with the WgPublicKey field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetWgPublicKey

`func (o *ClusterJoinRequest) SetWgPublicKey(v string)`

SetWgPublicKey sets WgPublicKey field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


