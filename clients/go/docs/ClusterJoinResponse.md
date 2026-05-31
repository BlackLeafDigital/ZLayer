# ClusterJoinResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**DekGeneration** | Pointer to **NullableInt64** | Cluster DEK generation that &#x60;wrapped_dek&#x60; was sealed under. Lets the joiner detect rotation drift if it re-joins after a revocation rotated the cluster DEK. &#x60;None&#x60; when &#x60;wrapped_dek&#x60; is &#x60;None&#x60;. | [optional] 
**JoinSecret** | Pointer to **NullableString** | Cluster-wide HMAC join secret. Returned to authenticated joiners so they can derive the same internal RPC bearer as the leader. &#x60;None&#x60; on legacy responses from older leaders. | [optional] 
**NodeId** | **string** | Assigned node UUID | 
**NodeJwt** | Pointer to **NullableString** | Node JWT minted by the leader for this joiner — &#x60;roles: [\&quot;node\&quot;]&#x60;, &#x60;node_id&#x60; set. Used to authenticate inter-node calls separately from any user identity. &#x60;None&#x60; on legacy responses. | [optional] 
**OverlayIp** | **string** | Assigned overlay IP for the new node | 
**Peers** | [**[]ClusterPeer**](ClusterPeer.md) | Existing peers in the cluster | 
**RaftNodeId** | **int64** | Assigned Raft node ID (monotonic u64) | 
**Role** | **string** | Role assigned to this node: \&quot;voter\&quot; or \&quot;learner\&quot; | 
**SliceCidr** | Pointer to **string** | Per-node slice CIDR assigned by the leader (e.g. \&quot;10.200.42.0/28\&quot;). Empty string if the leader is not slice-aware yet. | [optional] 
**Warnings** | Pointer to **[]string** | Server-side advisory warnings to surface to the operator/CLI.  Examples: \&quot;your token format is deprecated and will be removed in release X.Y\&quot;, \&quot;consider rotating the signing key, last rotated N days ago\&quot;. Present-but-empty means \&quot;no warnings\&quot;; serialized as &#x60;null&#x60; (skip-if-none) when there are none. | [optional] 
**WrappedDek** | Pointer to **[]int32** | Sealed-box-wrapped copy of the cluster DEK addressed to the joiner&#39;s &#x60;secrets_pubkey&#x60;. The joiner unwraps with its node X25519 private key and holds the DEK in zeroized memory. &#x60;None&#x60; on legacy responses or when the joiner did not provide a &#x60;secrets_pubkey&#x60;. | [optional] 

## Methods

### NewClusterJoinResponse

`func NewClusterJoinResponse(nodeId string, overlayIp string, peers []ClusterPeer, raftNodeId int64, role string, ) *ClusterJoinResponse`

NewClusterJoinResponse instantiates a new ClusterJoinResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewClusterJoinResponseWithDefaults

`func NewClusterJoinResponseWithDefaults() *ClusterJoinResponse`

NewClusterJoinResponseWithDefaults instantiates a new ClusterJoinResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetDekGeneration

`func (o *ClusterJoinResponse) GetDekGeneration() int64`

GetDekGeneration returns the DekGeneration field if non-nil, zero value otherwise.

### GetDekGenerationOk

`func (o *ClusterJoinResponse) GetDekGenerationOk() (*int64, bool)`

GetDekGenerationOk returns a tuple with the DekGeneration field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDekGeneration

`func (o *ClusterJoinResponse) SetDekGeneration(v int64)`

SetDekGeneration sets DekGeneration field to given value.

### HasDekGeneration

`func (o *ClusterJoinResponse) HasDekGeneration() bool`

HasDekGeneration returns a boolean if a field has been set.

### SetDekGenerationNil

`func (o *ClusterJoinResponse) SetDekGenerationNil(b bool)`

 SetDekGenerationNil sets the value for DekGeneration to be an explicit nil

### UnsetDekGeneration
`func (o *ClusterJoinResponse) UnsetDekGeneration()`

UnsetDekGeneration ensures that no value is present for DekGeneration, not even an explicit nil
### GetJoinSecret

`func (o *ClusterJoinResponse) GetJoinSecret() string`

GetJoinSecret returns the JoinSecret field if non-nil, zero value otherwise.

### GetJoinSecretOk

`func (o *ClusterJoinResponse) GetJoinSecretOk() (*string, bool)`

GetJoinSecretOk returns a tuple with the JoinSecret field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetJoinSecret

`func (o *ClusterJoinResponse) SetJoinSecret(v string)`

SetJoinSecret sets JoinSecret field to given value.

### HasJoinSecret

`func (o *ClusterJoinResponse) HasJoinSecret() bool`

HasJoinSecret returns a boolean if a field has been set.

### SetJoinSecretNil

`func (o *ClusterJoinResponse) SetJoinSecretNil(b bool)`

 SetJoinSecretNil sets the value for JoinSecret to be an explicit nil

### UnsetJoinSecret
`func (o *ClusterJoinResponse) UnsetJoinSecret()`

UnsetJoinSecret ensures that no value is present for JoinSecret, not even an explicit nil
### GetNodeId

`func (o *ClusterJoinResponse) GetNodeId() string`

GetNodeId returns the NodeId field if non-nil, zero value otherwise.

### GetNodeIdOk

`func (o *ClusterJoinResponse) GetNodeIdOk() (*string, bool)`

GetNodeIdOk returns a tuple with the NodeId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNodeId

`func (o *ClusterJoinResponse) SetNodeId(v string)`

SetNodeId sets NodeId field to given value.


### GetNodeJwt

`func (o *ClusterJoinResponse) GetNodeJwt() string`

GetNodeJwt returns the NodeJwt field if non-nil, zero value otherwise.

### GetNodeJwtOk

`func (o *ClusterJoinResponse) GetNodeJwtOk() (*string, bool)`

GetNodeJwtOk returns a tuple with the NodeJwt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNodeJwt

`func (o *ClusterJoinResponse) SetNodeJwt(v string)`

SetNodeJwt sets NodeJwt field to given value.

### HasNodeJwt

`func (o *ClusterJoinResponse) HasNodeJwt() bool`

HasNodeJwt returns a boolean if a field has been set.

### SetNodeJwtNil

`func (o *ClusterJoinResponse) SetNodeJwtNil(b bool)`

 SetNodeJwtNil sets the value for NodeJwt to be an explicit nil

### UnsetNodeJwt
`func (o *ClusterJoinResponse) UnsetNodeJwt()`

UnsetNodeJwt ensures that no value is present for NodeJwt, not even an explicit nil
### GetOverlayIp

`func (o *ClusterJoinResponse) GetOverlayIp() string`

GetOverlayIp returns the OverlayIp field if non-nil, zero value otherwise.

### GetOverlayIpOk

`func (o *ClusterJoinResponse) GetOverlayIpOk() (*string, bool)`

GetOverlayIpOk returns a tuple with the OverlayIp field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOverlayIp

`func (o *ClusterJoinResponse) SetOverlayIp(v string)`

SetOverlayIp sets OverlayIp field to given value.


### GetPeers

`func (o *ClusterJoinResponse) GetPeers() []ClusterPeer`

GetPeers returns the Peers field if non-nil, zero value otherwise.

### GetPeersOk

`func (o *ClusterJoinResponse) GetPeersOk() (*[]ClusterPeer, bool)`

GetPeersOk returns a tuple with the Peers field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPeers

`func (o *ClusterJoinResponse) SetPeers(v []ClusterPeer)`

SetPeers sets Peers field to given value.


### GetRaftNodeId

`func (o *ClusterJoinResponse) GetRaftNodeId() int64`

GetRaftNodeId returns the RaftNodeId field if non-nil, zero value otherwise.

### GetRaftNodeIdOk

`func (o *ClusterJoinResponse) GetRaftNodeIdOk() (*int64, bool)`

GetRaftNodeIdOk returns a tuple with the RaftNodeId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRaftNodeId

`func (o *ClusterJoinResponse) SetRaftNodeId(v int64)`

SetRaftNodeId sets RaftNodeId field to given value.


### GetRole

`func (o *ClusterJoinResponse) GetRole() string`

GetRole returns the Role field if non-nil, zero value otherwise.

### GetRoleOk

`func (o *ClusterJoinResponse) GetRoleOk() (*string, bool)`

GetRoleOk returns a tuple with the Role field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRole

`func (o *ClusterJoinResponse) SetRole(v string)`

SetRole sets Role field to given value.


### GetSliceCidr

`func (o *ClusterJoinResponse) GetSliceCidr() string`

GetSliceCidr returns the SliceCidr field if non-nil, zero value otherwise.

### GetSliceCidrOk

`func (o *ClusterJoinResponse) GetSliceCidrOk() (*string, bool)`

GetSliceCidrOk returns a tuple with the SliceCidr field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSliceCidr

`func (o *ClusterJoinResponse) SetSliceCidr(v string)`

SetSliceCidr sets SliceCidr field to given value.

### HasSliceCidr

`func (o *ClusterJoinResponse) HasSliceCidr() bool`

HasSliceCidr returns a boolean if a field has been set.

### GetWarnings

`func (o *ClusterJoinResponse) GetWarnings() []string`

GetWarnings returns the Warnings field if non-nil, zero value otherwise.

### GetWarningsOk

`func (o *ClusterJoinResponse) GetWarningsOk() (*[]string, bool)`

GetWarningsOk returns a tuple with the Warnings field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetWarnings

`func (o *ClusterJoinResponse) SetWarnings(v []string)`

SetWarnings sets Warnings field to given value.

### HasWarnings

`func (o *ClusterJoinResponse) HasWarnings() bool`

HasWarnings returns a boolean if a field has been set.

### SetWarningsNil

`func (o *ClusterJoinResponse) SetWarningsNil(b bool)`

 SetWarningsNil sets the value for Warnings to be an explicit nil

### UnsetWarnings
`func (o *ClusterJoinResponse) UnsetWarnings()`

UnsetWarnings ensures that no value is present for Warnings, not even an explicit nil
### GetWrappedDek

`func (o *ClusterJoinResponse) GetWrappedDek() []int32`

GetWrappedDek returns the WrappedDek field if non-nil, zero value otherwise.

### GetWrappedDekOk

`func (o *ClusterJoinResponse) GetWrappedDekOk() (*[]int32, bool)`

GetWrappedDekOk returns a tuple with the WrappedDek field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetWrappedDek

`func (o *ClusterJoinResponse) SetWrappedDek(v []int32)`

SetWrappedDek sets WrappedDek field to given value.

### HasWrappedDek

`func (o *ClusterJoinResponse) HasWrappedDek() bool`

HasWrappedDek returns a boolean if a field has been set.

### SetWrappedDekNil

`func (o *ClusterJoinResponse) SetWrappedDekNil(b bool)`

 SetWrappedDekNil sets the value for WrappedDek to be an explicit nil

### UnsetWrappedDek
`func (o *ClusterJoinResponse) UnsetWrappedDek()`

UnsetWrappedDek ensures that no value is present for WrappedDek, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


