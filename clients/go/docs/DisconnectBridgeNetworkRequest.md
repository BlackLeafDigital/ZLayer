# DisconnectBridgeNetworkRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**ContainerId** | **string** | Container id to detach. | 
**Force** | Pointer to **bool** | If true, the runtime is asked to forcibly detach. | [optional] 

## Methods

### NewDisconnectBridgeNetworkRequest

`func NewDisconnectBridgeNetworkRequest(containerId string, ) *DisconnectBridgeNetworkRequest`

NewDisconnectBridgeNetworkRequest instantiates a new DisconnectBridgeNetworkRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewDisconnectBridgeNetworkRequestWithDefaults

`func NewDisconnectBridgeNetworkRequestWithDefaults() *DisconnectBridgeNetworkRequest`

NewDisconnectBridgeNetworkRequestWithDefaults instantiates a new DisconnectBridgeNetworkRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetContainerId

`func (o *DisconnectBridgeNetworkRequest) GetContainerId() string`

GetContainerId returns the ContainerId field if non-nil, zero value otherwise.

### GetContainerIdOk

`func (o *DisconnectBridgeNetworkRequest) GetContainerIdOk() (*string, bool)`

GetContainerIdOk returns a tuple with the ContainerId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetContainerId

`func (o *DisconnectBridgeNetworkRequest) SetContainerId(v string)`

SetContainerId sets ContainerId field to given value.


### GetForce

`func (o *DisconnectBridgeNetworkRequest) GetForce() bool`

GetForce returns the Force field if non-nil, zero value otherwise.

### GetForceOk

`func (o *DisconnectBridgeNetworkRequest) GetForceOk() (*bool, bool)`

GetForceOk returns a tuple with the Force field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetForce

`func (o *DisconnectBridgeNetworkRequest) SetForce(v bool)`

SetForce sets Force field to given value.

### HasForce

`func (o *DisconnectBridgeNetworkRequest) HasForce() bool`

HasForce returns a boolean if a field has been set.


[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


