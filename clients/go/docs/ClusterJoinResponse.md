# ClusterJoinResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**NodeId** | **string** | Assigned node UUID | 
**OverlayIp** | **string** | Assigned overlay IP for the new node | 
**Peers** | [**[]ClusterPeer**](ClusterPeer.md) | Existing peers in the cluster | 
**RaftNodeId** | **int64** | Assigned Raft node ID (monotonic u64) | 
**Role** | **string** | Role assigned to this node: \&quot;voter\&quot; or \&quot;learner\&quot; | 

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



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


