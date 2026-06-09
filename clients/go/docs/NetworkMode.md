# NetworkMode

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Bridge** | [**NetworkModeOneOfBridge**](NetworkModeOneOfBridge.md) |  | 
**Container** | [**NetworkModeOneOf1Container**](NetworkModeOneOf1Container.md) |  | 

## Methods

### NewNetworkMode

`func NewNetworkMode(bridge NetworkModeOneOfBridge, container NetworkModeOneOf1Container, ) *NetworkMode`

NewNetworkMode instantiates a new NetworkMode object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewNetworkModeWithDefaults

`func NewNetworkModeWithDefaults() *NetworkMode`

NewNetworkModeWithDefaults instantiates a new NetworkMode object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetBridge

`func (o *NetworkMode) GetBridge() NetworkModeOneOfBridge`

GetBridge returns the Bridge field if non-nil, zero value otherwise.

### GetBridgeOk

`func (o *NetworkMode) GetBridgeOk() (*NetworkModeOneOfBridge, bool)`

GetBridgeOk returns a tuple with the Bridge field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBridge

`func (o *NetworkMode) SetBridge(v NetworkModeOneOfBridge)`

SetBridge sets Bridge field to given value.


### GetContainer

`func (o *NetworkMode) GetContainer() NetworkModeOneOf1Container`

GetContainer returns the Container field if non-nil, zero value otherwise.

### GetContainerOk

`func (o *NetworkMode) GetContainerOk() (*NetworkModeOneOf1Container, bool)`

GetContainerOk returns a tuple with the Container field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetContainer

`func (o *NetworkMode) SetContainer(v NetworkModeOneOf1Container)`

SetContainer sets Container field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


