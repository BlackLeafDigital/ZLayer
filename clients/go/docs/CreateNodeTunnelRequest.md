# CreateNodeTunnelRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Expose** | Pointer to **string** | Exposure level (public, internal) | [optional] 
**FromNode** | **string** | Source node ID | 
**LocalPort** | **int32** | Local port on the source node | 
**Name** | **string** | Name for this tunnel | 
**RemotePort** | **int32** | Remote port on the destination node | 
**ToNode** | **string** | Destination node ID | 

## Methods

### NewCreateNodeTunnelRequest

`func NewCreateNodeTunnelRequest(fromNode string, localPort int32, name string, remotePort int32, toNode string, ) *CreateNodeTunnelRequest`

NewCreateNodeTunnelRequest instantiates a new CreateNodeTunnelRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateNodeTunnelRequestWithDefaults

`func NewCreateNodeTunnelRequestWithDefaults() *CreateNodeTunnelRequest`

NewCreateNodeTunnelRequestWithDefaults instantiates a new CreateNodeTunnelRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetExpose

`func (o *CreateNodeTunnelRequest) GetExpose() string`

GetExpose returns the Expose field if non-nil, zero value otherwise.

### GetExposeOk

`func (o *CreateNodeTunnelRequest) GetExposeOk() (*string, bool)`

GetExposeOk returns a tuple with the Expose field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExpose

`func (o *CreateNodeTunnelRequest) SetExpose(v string)`

SetExpose sets Expose field to given value.

### HasExpose

`func (o *CreateNodeTunnelRequest) HasExpose() bool`

HasExpose returns a boolean if a field has been set.

### GetFromNode

`func (o *CreateNodeTunnelRequest) GetFromNode() string`

GetFromNode returns the FromNode field if non-nil, zero value otherwise.

### GetFromNodeOk

`func (o *CreateNodeTunnelRequest) GetFromNodeOk() (*string, bool)`

GetFromNodeOk returns a tuple with the FromNode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetFromNode

`func (o *CreateNodeTunnelRequest) SetFromNode(v string)`

SetFromNode sets FromNode field to given value.


### GetLocalPort

`func (o *CreateNodeTunnelRequest) GetLocalPort() int32`

GetLocalPort returns the LocalPort field if non-nil, zero value otherwise.

### GetLocalPortOk

`func (o *CreateNodeTunnelRequest) GetLocalPortOk() (*int32, bool)`

GetLocalPortOk returns a tuple with the LocalPort field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLocalPort

`func (o *CreateNodeTunnelRequest) SetLocalPort(v int32)`

SetLocalPort sets LocalPort field to given value.


### GetName

`func (o *CreateNodeTunnelRequest) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *CreateNodeTunnelRequest) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *CreateNodeTunnelRequest) SetName(v string)`

SetName sets Name field to given value.


### GetRemotePort

`func (o *CreateNodeTunnelRequest) GetRemotePort() int32`

GetRemotePort returns the RemotePort field if non-nil, zero value otherwise.

### GetRemotePortOk

`func (o *CreateNodeTunnelRequest) GetRemotePortOk() (*int32, bool)`

GetRemotePortOk returns a tuple with the RemotePort field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRemotePort

`func (o *CreateNodeTunnelRequest) SetRemotePort(v int32)`

SetRemotePort sets RemotePort field to given value.


### GetToNode

`func (o *CreateNodeTunnelRequest) GetToNode() string`

GetToNode returns the ToNode field if non-nil, zero value otherwise.

### GetToNodeOk

`func (o *CreateNodeTunnelRequest) GetToNodeOk() (*string, bool)`

GetToNodeOk returns a tuple with the ToNode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetToNode

`func (o *CreateNodeTunnelRequest) SetToNode(v string)`

SetToNode sets ToNode field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


