# CreateNodeTunnelResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Expose** | **string** | Exposure level | 
**FromNode** | **string** | Source node ID | 
**LocalPort** | **int32** | Local port | 
**Name** | **string** | Tunnel name | 
**RemotePort** | **int32** | Remote port | 
**Status** | **string** | Current status | 
**ToNode** | **string** | Destination node ID | 

## Methods

### NewCreateNodeTunnelResponse

`func NewCreateNodeTunnelResponse(expose string, fromNode string, localPort int32, name string, remotePort int32, status string, toNode string, ) *CreateNodeTunnelResponse`

NewCreateNodeTunnelResponse instantiates a new CreateNodeTunnelResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateNodeTunnelResponseWithDefaults

`func NewCreateNodeTunnelResponseWithDefaults() *CreateNodeTunnelResponse`

NewCreateNodeTunnelResponseWithDefaults instantiates a new CreateNodeTunnelResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetExpose

`func (o *CreateNodeTunnelResponse) GetExpose() string`

GetExpose returns the Expose field if non-nil, zero value otherwise.

### GetExposeOk

`func (o *CreateNodeTunnelResponse) GetExposeOk() (*string, bool)`

GetExposeOk returns a tuple with the Expose field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExpose

`func (o *CreateNodeTunnelResponse) SetExpose(v string)`

SetExpose sets Expose field to given value.


### GetFromNode

`func (o *CreateNodeTunnelResponse) GetFromNode() string`

GetFromNode returns the FromNode field if non-nil, zero value otherwise.

### GetFromNodeOk

`func (o *CreateNodeTunnelResponse) GetFromNodeOk() (*string, bool)`

GetFromNodeOk returns a tuple with the FromNode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetFromNode

`func (o *CreateNodeTunnelResponse) SetFromNode(v string)`

SetFromNode sets FromNode field to given value.


### GetLocalPort

`func (o *CreateNodeTunnelResponse) GetLocalPort() int32`

GetLocalPort returns the LocalPort field if non-nil, zero value otherwise.

### GetLocalPortOk

`func (o *CreateNodeTunnelResponse) GetLocalPortOk() (*int32, bool)`

GetLocalPortOk returns a tuple with the LocalPort field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLocalPort

`func (o *CreateNodeTunnelResponse) SetLocalPort(v int32)`

SetLocalPort sets LocalPort field to given value.


### GetName

`func (o *CreateNodeTunnelResponse) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *CreateNodeTunnelResponse) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *CreateNodeTunnelResponse) SetName(v string)`

SetName sets Name field to given value.


### GetRemotePort

`func (o *CreateNodeTunnelResponse) GetRemotePort() int32`

GetRemotePort returns the RemotePort field if non-nil, zero value otherwise.

### GetRemotePortOk

`func (o *CreateNodeTunnelResponse) GetRemotePortOk() (*int32, bool)`

GetRemotePortOk returns a tuple with the RemotePort field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRemotePort

`func (o *CreateNodeTunnelResponse) SetRemotePort(v int32)`

SetRemotePort sets RemotePort field to given value.


### GetStatus

`func (o *CreateNodeTunnelResponse) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *CreateNodeTunnelResponse) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *CreateNodeTunnelResponse) SetStatus(v string)`

SetStatus sets Status field to given value.


### GetToNode

`func (o *CreateNodeTunnelResponse) GetToNode() string`

GetToNode returns the ToNode field if non-nil, zero value otherwise.

### GetToNodeOk

`func (o *CreateNodeTunnelResponse) GetToNodeOk() (*string, bool)`

GetToNodeOk returns a tuple with the ToNode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetToNode

`func (o *CreateNodeTunnelResponse) SetToNode(v string)`

SetToNode sets ToNode field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


