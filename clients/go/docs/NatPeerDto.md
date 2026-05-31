# NatPeerDto

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**ConnectionType** | **string** | Direct / &#x60;HolePunched&#x60; / Relayed / Unreachable | 
**NodeId** | **string** | Peer node ID | 
**RemoteEndpoint** | Pointer to **NullableString** | Selected remote endpoint, if any | [optional] 

## Methods

### NewNatPeerDto

`func NewNatPeerDto(connectionType string, nodeId string, ) *NatPeerDto`

NewNatPeerDto instantiates a new NatPeerDto object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewNatPeerDtoWithDefaults

`func NewNatPeerDtoWithDefaults() *NatPeerDto`

NewNatPeerDtoWithDefaults instantiates a new NatPeerDto object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetConnectionType

`func (o *NatPeerDto) GetConnectionType() string`

GetConnectionType returns the ConnectionType field if non-nil, zero value otherwise.

### GetConnectionTypeOk

`func (o *NatPeerDto) GetConnectionTypeOk() (*string, bool)`

GetConnectionTypeOk returns a tuple with the ConnectionType field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetConnectionType

`func (o *NatPeerDto) SetConnectionType(v string)`

SetConnectionType sets ConnectionType field to given value.


### GetNodeId

`func (o *NatPeerDto) GetNodeId() string`

GetNodeId returns the NodeId field if non-nil, zero value otherwise.

### GetNodeIdOk

`func (o *NatPeerDto) GetNodeIdOk() (*string, bool)`

GetNodeIdOk returns a tuple with the NodeId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNodeId

`func (o *NatPeerDto) SetNodeId(v string)`

SetNodeId sets NodeId field to given value.


### GetRemoteEndpoint

`func (o *NatPeerDto) GetRemoteEndpoint() string`

GetRemoteEndpoint returns the RemoteEndpoint field if non-nil, zero value otherwise.

### GetRemoteEndpointOk

`func (o *NatPeerDto) GetRemoteEndpointOk() (*string, bool)`

GetRemoteEndpointOk returns a tuple with the RemoteEndpoint field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRemoteEndpoint

`func (o *NatPeerDto) SetRemoteEndpoint(v string)`

SetRemoteEndpoint sets RemoteEndpoint field to given value.

### HasRemoteEndpoint

`func (o *NatPeerDto) HasRemoteEndpoint() bool`

HasRemoteEndpoint returns a boolean if a field has been set.

### SetRemoteEndpointNil

`func (o *NatPeerDto) SetRemoteEndpointNil(b bool)`

 SetRemoteEndpointNil sets the value for RemoteEndpoint to be an explicit nil

### UnsetRemoteEndpoint
`func (o *NatPeerDto) UnsetRemoteEndpoint()`

UnsetRemoteEndpoint ensures that no value is present for RemoteEndpoint, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


