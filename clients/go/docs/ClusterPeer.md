# ClusterPeer

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**AdvertiseAddr** | **string** | Advertise address | 
**NodeId** | **string** | UUID | 
**OverlayIp** | **string** | Overlay IP | 
**OverlayPort** | **int32** | Overlay port | 
**RaftNodeId** | **int64** | Raft node ID | 
**RaftPort** | **int32** | Raft port | 
**WgPublicKey** | **string** | &#x60;WireGuard&#x60; public key | 

## Methods

### NewClusterPeer

`func NewClusterPeer(advertiseAddr string, nodeId string, overlayIp string, overlayPort int32, raftNodeId int64, raftPort int32, wgPublicKey string, ) *ClusterPeer`

NewClusterPeer instantiates a new ClusterPeer object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewClusterPeerWithDefaults

`func NewClusterPeerWithDefaults() *ClusterPeer`

NewClusterPeerWithDefaults instantiates a new ClusterPeer object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAdvertiseAddr

`func (o *ClusterPeer) GetAdvertiseAddr() string`

GetAdvertiseAddr returns the AdvertiseAddr field if non-nil, zero value otherwise.

### GetAdvertiseAddrOk

`func (o *ClusterPeer) GetAdvertiseAddrOk() (*string, bool)`

GetAdvertiseAddrOk returns a tuple with the AdvertiseAddr field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAdvertiseAddr

`func (o *ClusterPeer) SetAdvertiseAddr(v string)`

SetAdvertiseAddr sets AdvertiseAddr field to given value.


### GetNodeId

`func (o *ClusterPeer) GetNodeId() string`

GetNodeId returns the NodeId field if non-nil, zero value otherwise.

### GetNodeIdOk

`func (o *ClusterPeer) GetNodeIdOk() (*string, bool)`

GetNodeIdOk returns a tuple with the NodeId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNodeId

`func (o *ClusterPeer) SetNodeId(v string)`

SetNodeId sets NodeId field to given value.


### GetOverlayIp

`func (o *ClusterPeer) GetOverlayIp() string`

GetOverlayIp returns the OverlayIp field if non-nil, zero value otherwise.

### GetOverlayIpOk

`func (o *ClusterPeer) GetOverlayIpOk() (*string, bool)`

GetOverlayIpOk returns a tuple with the OverlayIp field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOverlayIp

`func (o *ClusterPeer) SetOverlayIp(v string)`

SetOverlayIp sets OverlayIp field to given value.


### GetOverlayPort

`func (o *ClusterPeer) GetOverlayPort() int32`

GetOverlayPort returns the OverlayPort field if non-nil, zero value otherwise.

### GetOverlayPortOk

`func (o *ClusterPeer) GetOverlayPortOk() (*int32, bool)`

GetOverlayPortOk returns a tuple with the OverlayPort field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOverlayPort

`func (o *ClusterPeer) SetOverlayPort(v int32)`

SetOverlayPort sets OverlayPort field to given value.


### GetRaftNodeId

`func (o *ClusterPeer) GetRaftNodeId() int64`

GetRaftNodeId returns the RaftNodeId field if non-nil, zero value otherwise.

### GetRaftNodeIdOk

`func (o *ClusterPeer) GetRaftNodeIdOk() (*int64, bool)`

GetRaftNodeIdOk returns a tuple with the RaftNodeId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRaftNodeId

`func (o *ClusterPeer) SetRaftNodeId(v int64)`

SetRaftNodeId sets RaftNodeId field to given value.


### GetRaftPort

`func (o *ClusterPeer) GetRaftPort() int32`

GetRaftPort returns the RaftPort field if non-nil, zero value otherwise.

### GetRaftPortOk

`func (o *ClusterPeer) GetRaftPortOk() (*int32, bool)`

GetRaftPortOk returns a tuple with the RaftPort field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRaftPort

`func (o *ClusterPeer) SetRaftPort(v int32)`

SetRaftPort sets RaftPort field to given value.


### GetWgPublicKey

`func (o *ClusterPeer) GetWgPublicKey() string`

GetWgPublicKey returns the WgPublicKey field if non-nil, zero value otherwise.

### GetWgPublicKeyOk

`func (o *ClusterPeer) GetWgPublicKeyOk() (*string, bool)`

GetWgPublicKeyOk returns a tuple with the WgPublicKey field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetWgPublicKey

`func (o *ClusterPeer) SetWgPublicKey(v string)`

SetWgPublicKey sets WgPublicKey field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


