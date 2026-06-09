# NetworkEvent

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**At** | **time.Time** | Wall-clock time. | 
**ContainerId** | Pointer to **NullableString** | Container id, populated for &#x60;Connect&#x60;/&#x60;Disconnect&#x60;. | [optional] 
**Driver** | Pointer to **string** | Network driver (e.g. &#x60;bridge&#x60;, &#x60;overlay&#x60;). | [optional] 
**Id** | **string** | Network identifier (registry id). | 
**Kind** | [**NetworkEventKind**](NetworkEventKind.md) |  | 
**Name** | **string** | Human-readable network name. | 

## Methods

### NewNetworkEvent

`func NewNetworkEvent(at time.Time, id string, kind NetworkEventKind, name string, ) *NetworkEvent`

NewNetworkEvent instantiates a new NetworkEvent object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewNetworkEventWithDefaults

`func NewNetworkEventWithDefaults() *NetworkEvent`

NewNetworkEventWithDefaults instantiates a new NetworkEvent object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAt

`func (o *NetworkEvent) GetAt() time.Time`

GetAt returns the At field if non-nil, zero value otherwise.

### GetAtOk

`func (o *NetworkEvent) GetAtOk() (*time.Time, bool)`

GetAtOk returns a tuple with the At field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAt

`func (o *NetworkEvent) SetAt(v time.Time)`

SetAt sets At field to given value.


### GetContainerId

`func (o *NetworkEvent) GetContainerId() string`

GetContainerId returns the ContainerId field if non-nil, zero value otherwise.

### GetContainerIdOk

`func (o *NetworkEvent) GetContainerIdOk() (*string, bool)`

GetContainerIdOk returns a tuple with the ContainerId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetContainerId

`func (o *NetworkEvent) SetContainerId(v string)`

SetContainerId sets ContainerId field to given value.

### HasContainerId

`func (o *NetworkEvent) HasContainerId() bool`

HasContainerId returns a boolean if a field has been set.

### SetContainerIdNil

`func (o *NetworkEvent) SetContainerIdNil(b bool)`

 SetContainerIdNil sets the value for ContainerId to be an explicit nil

### UnsetContainerId
`func (o *NetworkEvent) UnsetContainerId()`

UnsetContainerId ensures that no value is present for ContainerId, not even an explicit nil
### GetDriver

`func (o *NetworkEvent) GetDriver() string`

GetDriver returns the Driver field if non-nil, zero value otherwise.

### GetDriverOk

`func (o *NetworkEvent) GetDriverOk() (*string, bool)`

GetDriverOk returns a tuple with the Driver field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDriver

`func (o *NetworkEvent) SetDriver(v string)`

SetDriver sets Driver field to given value.

### HasDriver

`func (o *NetworkEvent) HasDriver() bool`

HasDriver returns a boolean if a field has been set.

### GetId

`func (o *NetworkEvent) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *NetworkEvent) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *NetworkEvent) SetId(v string)`

SetId sets Id field to given value.


### GetKind

`func (o *NetworkEvent) GetKind() NetworkEventKind`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *NetworkEvent) GetKindOk() (*NetworkEventKind, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *NetworkEvent) SetKind(v NetworkEventKind)`

SetKind sets Kind field to given value.


### GetName

`func (o *NetworkEvent) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *NetworkEvent) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *NetworkEvent) SetName(v string)`

SetName sets Name field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


