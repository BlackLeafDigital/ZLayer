# GossipPeerSummary

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Labels** | Pointer to **map[string]string** | Free-form labels advertised by the peer. | [optional] 
**NodeId** | **int64** | Worker (or peer) node id. | 
**OverlayIp** | Pointer to **NullableString** | Overlay IP assigned to this peer, if known. | [optional] 
**WgEndpoint** | Pointer to **NullableString** | &#x60;WireGuard&#x60; UDP endpoint (host:port), if known. | [optional] 
**WgPubkey** | Pointer to **NullableString** | &#x60;WireGuard&#x60; public key (base64-url-no-pad), if known. | [optional] 

## Methods

### NewGossipPeerSummary

`func NewGossipPeerSummary(nodeId int64, ) *GossipPeerSummary`

NewGossipPeerSummary instantiates a new GossipPeerSummary object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewGossipPeerSummaryWithDefaults

`func NewGossipPeerSummaryWithDefaults() *GossipPeerSummary`

NewGossipPeerSummaryWithDefaults instantiates a new GossipPeerSummary object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetLabels

`func (o *GossipPeerSummary) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *GossipPeerSummary) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *GossipPeerSummary) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.

### HasLabels

`func (o *GossipPeerSummary) HasLabels() bool`

HasLabels returns a boolean if a field has been set.

### GetNodeId

`func (o *GossipPeerSummary) GetNodeId() int64`

GetNodeId returns the NodeId field if non-nil, zero value otherwise.

### GetNodeIdOk

`func (o *GossipPeerSummary) GetNodeIdOk() (*int64, bool)`

GetNodeIdOk returns a tuple with the NodeId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNodeId

`func (o *GossipPeerSummary) SetNodeId(v int64)`

SetNodeId sets NodeId field to given value.


### GetOverlayIp

`func (o *GossipPeerSummary) GetOverlayIp() string`

GetOverlayIp returns the OverlayIp field if non-nil, zero value otherwise.

### GetOverlayIpOk

`func (o *GossipPeerSummary) GetOverlayIpOk() (*string, bool)`

GetOverlayIpOk returns a tuple with the OverlayIp field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOverlayIp

`func (o *GossipPeerSummary) SetOverlayIp(v string)`

SetOverlayIp sets OverlayIp field to given value.

### HasOverlayIp

`func (o *GossipPeerSummary) HasOverlayIp() bool`

HasOverlayIp returns a boolean if a field has been set.

### SetOverlayIpNil

`func (o *GossipPeerSummary) SetOverlayIpNil(b bool)`

 SetOverlayIpNil sets the value for OverlayIp to be an explicit nil

### UnsetOverlayIp
`func (o *GossipPeerSummary) UnsetOverlayIp()`

UnsetOverlayIp ensures that no value is present for OverlayIp, not even an explicit nil
### GetWgEndpoint

`func (o *GossipPeerSummary) GetWgEndpoint() string`

GetWgEndpoint returns the WgEndpoint field if non-nil, zero value otherwise.

### GetWgEndpointOk

`func (o *GossipPeerSummary) GetWgEndpointOk() (*string, bool)`

GetWgEndpointOk returns a tuple with the WgEndpoint field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetWgEndpoint

`func (o *GossipPeerSummary) SetWgEndpoint(v string)`

SetWgEndpoint sets WgEndpoint field to given value.

### HasWgEndpoint

`func (o *GossipPeerSummary) HasWgEndpoint() bool`

HasWgEndpoint returns a boolean if a field has been set.

### SetWgEndpointNil

`func (o *GossipPeerSummary) SetWgEndpointNil(b bool)`

 SetWgEndpointNil sets the value for WgEndpoint to be an explicit nil

### UnsetWgEndpoint
`func (o *GossipPeerSummary) UnsetWgEndpoint()`

UnsetWgEndpoint ensures that no value is present for WgEndpoint, not even an explicit nil
### GetWgPubkey

`func (o *GossipPeerSummary) GetWgPubkey() string`

GetWgPubkey returns the WgPubkey field if non-nil, zero value otherwise.

### GetWgPubkeyOk

`func (o *GossipPeerSummary) GetWgPubkeyOk() (*string, bool)`

GetWgPubkeyOk returns a tuple with the WgPubkey field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetWgPubkey

`func (o *GossipPeerSummary) SetWgPubkey(v string)`

SetWgPubkey sets WgPubkey field to given value.

### HasWgPubkey

`func (o *GossipPeerSummary) HasWgPubkey() bool`

HasWgPubkey returns a boolean if a field has been set.

### SetWgPubkeyNil

`func (o *GossipPeerSummary) SetWgPubkeyNil(b bool)`

 SetWgPubkeyNil sets the value for WgPubkey to be an explicit nil

### UnsetWgPubkey
`func (o *GossipPeerSummary) UnsetWgPubkey()`

UnsetWgPubkey ensures that no value is present for WgPubkey, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


